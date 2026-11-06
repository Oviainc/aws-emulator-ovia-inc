// SPDX-License-Identifier: CC0-1.0

//! The stored RECORDS of this service's model, endpoint and job resources: a marketplace
//! endpoint, an imported model, an evaluation job, a prompt optimization job, a Provisioned
//! Throughput, a custom model and its deployment, a copy job, a customization job and a
//! batch inference job — each with the lifecycle its own clock owes it.

use super::*;

/// One stored Bedrock Marketplace model endpoint: the identity
/// `CreateMarketplaceModelEndpoint` minted, the configuration the latest call carried, the
/// two instants a read reports, and the endpoint lifecycle status.
///
/// `status` is `&'static str` for the same reason [`PromptRouter::status`] is: it is the
/// single value this emulator mints ([`MARKETPLACE_ENDPOINT_REGISTERED`]) rather than
/// anything a request chooses.
#[derive(Debug, Clone)]
pub struct MarketplaceModelEndpointRecord {
    /// The `endpointArn` this endpoint is addressed by, minted by
    /// [`marketplace_model_endpoint_arn`].
    pub arn: String,
    /// `endpointName`, as the create request spelled it. The ARN's resource id, so it is
    /// also what makes the name unique within the account and region.
    pub name: String,
    /// `modelSourceIdentifier` — the Marketplace model this endpoint serves, as the create
    /// request named it and as each `RegisterMarketplaceModelEndpoint` re-states it: the
    /// registration is where the model deployed on the endpoint is recorded.
    pub model_source_identifier: String,
    /// `endpointConfig` — the create request's configuration, REPLACED whole by each
    /// `UpdateMarketplaceModelEndpoint`.
    pub endpoint_config: EndpointConfig,
    /// `acceptEula`, as the create request left it (the member defaults to `false`).
    ///
    /// Kept with the record because it is part of the request that minted the endpoint; no
    /// response member of this service reports it, so nothing reads it back today.
    pub accept_eula: bool,
    pub tags: Vec<Tag>,
    /// `MarketplaceModelEndpoint$status`; [`MARKETPLACE_ENDPOINT_REGISTERED`] for every
    /// endpoint minted here.
    pub status: &'static str,
    /// `MarketplaceModelEndpoint$endpointStatus` — the endpoint's own lifecycle status.
    pub endpoint_status: &'static str,
    /// When [`Self::endpoint_status`] is owed its move to
    /// [`ENDPOINT_STATUS_IN_SERVICE`], or `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer: [`Self::settle`] materialises the move on the next
    /// call that touches the endpoint, so the transition is owed at the same instant
    /// whether or not anybody calls, and no handler sleeps for it.
    pub in_service_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the endpoint's configuration last changed: the creation instant until an
    /// `UpdateMarketplaceModelEndpoint` moves it.
    pub updated_at: SystemTime,
    /// The `clientRequestToken` of the most recent `UpdateMarketplaceModelEndpoint`
    /// applied to this endpoint, so a RETRY of that same call — the request the
    /// `@idempotencyToken` member is there for — applies nothing a second time.
    ///
    /// ONE token per endpoint rather than a map of every token ever seen, so this cannot
    /// grow with the number of updates a caller issues.
    pub last_update_token: Option<String>,
}

impl MarketplaceModelEndpointRecord {
    /// Start a non-terminal `endpointStatus`, due to settle
    /// [`ENDPOINT_SETTLE_AFTER`] from `now`.
    pub fn begin(&mut self, status: &'static str, now: SystemTime) {
        self.endpoint_status = status;
        self.in_service_due_at = Some(now + ENDPOINT_SETTLE_AFTER);
    }

    /// Materialise the move to [`ENDPOINT_STATUS_IN_SERVICE`] the clock has already made
    /// due at `now`, and report the status to report.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at `now`
    /// even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.in_service_due_at.is_some_and(|due| now >= due) {
            self.endpoint_status = ENDPOINT_STATUS_IN_SERVICE;
            self.in_service_due_at = None;
        }
    }
}

/// One stored model import job: the identity `CreateModelImportJob` minted, the request
/// that minted it, the instants a read reports, and the job's own lifecycle status.
///
/// `status` is `&'static str` for the same reason [`PromptRouter::status`] is: this
/// emulator mints only the two values the lifecycle passes through
/// ([`IMPORT_JOB_STATUS_IN_PROGRESS`], [`IMPORT_JOB_STATUS_COMPLETED`]) rather than
/// anything a request chooses.
#[derive(Debug, Clone)]
pub struct ModelImportJobRecord {
    /// The `ModelImportJobArn` this job is addressed by, minted by
    /// [`model_import_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it — the other spelling a
    /// `ModelImportJobIdentifier` may carry.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index and the uniqueness checks are per region without re-parsing an
    /// ARN.
    pub region: String,
    /// `importedModelName` — the name of the model this job imports.
    pub imported_model_name: String,
    /// The `ImportedModelArn` of the model this job imports, minted by
    /// [`imported_model_arn`] when the job is accepted: it is the job's target, so it is
    /// fixed from that call onwards.
    pub imported_model_arn: String,
    /// `roleArn` — the service role Amazon Bedrock assumes for the import.
    pub role_arn: String,
    /// `modelDataSource` — where the model being imported is read from.
    pub model_data_source: ModelDataSource,
    /// `vpcConfig`, as the create request carried it (absent when it carried none).
    pub vpc_config: Option<VpcConfig>,
    /// The `importedModelKmsKeyArn` a read reports for the request's
    /// `importedModelKmsKeyId`, as [`kms_key_arn`] completes it.
    pub imported_model_kms_key_arn: Option<String>,
    /// `jobTags` — the tags attached to the import job itself.
    pub job_tags: Vec<Tag>,
    /// `importedModelTags` — the tags attached to the model the job imports.
    pub imported_model_tags: Vec<Tag>,
    /// `ModelImportJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`IMPORT_JOB_STATUS_COMPLETED`], or
    /// `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed: the creation instant until the import completes.
    pub last_modified_at: SystemTime,
    /// When the job ended, or `None` while it is still running.
    pub ended_at: Option<SystemTime>,
}

impl ModelImportJobRecord {
    /// Materialise the move to [`IMPORT_JOB_STATUS_COMPLETED`] the clock has already made
    /// due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its end.
    pub fn complete(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = IMPORT_JOB_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.ended_at = Some(due);
            self.last_modified_at = due;
        }
    }
}

/// One stored evaluation job: the identity `CreateEvaluationJob` minted, the
/// configuration that request carried, and the instants a read reports.
///
/// `status` and `job_type` are `&'static str` for the same reason
/// [`ModelImportJobRecord::status`] is: both are values this emulator derives
/// ([`EVALUATION_JOB_STATUS_IN_PROGRESS`], [`evaluation_job_type_of`]) rather than
/// anything a request names directly.
#[derive(Debug, Clone)]
pub struct EvaluationJobRecord {
    /// The `EvaluationJobArn` this job is addressed by, minted by [`evaluation_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN.
    pub region: String,
    /// `jobDescription`, as the create request carried it (absent when it carried none).
    pub description: Option<String>,
    /// `roleArn` — the service role Amazon Bedrock assumes to run the evaluation.
    pub role_arn: String,
    /// `customerEncryptionKeyId`, as the create request carried it. Stored and reported
    /// VERBATIM: the read's `customerEncryptionKeyId` carries the same `KmsKeyId` shape
    /// the request's does, so a key ARN, a bare key id and an alias all come back as they
    /// were given (unlike a member a read reports as a `KmsKeyArn`, which must be
    /// completed by [`kms_key_arn`]).
    pub customer_encryption_key_id: Option<String>,
    /// `applicationType`, as the create request carried it (absent when it carried none —
    /// the member is optional in both the request and the response, and the model declares
    /// no default for it).
    pub application_type: Option<String>,
    /// `evaluationConfig` — the automated or human evaluation the job runs.
    pub evaluation_config: EvaluationConfig,
    /// `inferenceConfig` — the models or knowledge bases the job evaluates.
    pub inference_config: EvaluationInferenceConfig,
    /// `outputDataConfig` — where the job's results are saved.
    pub output_data_config: EvaluationOutputDataConfig,
    /// `jobTags` — the tags attached to the evaluation job.
    pub tags: Vec<Tag>,
    /// `EvaluationJobType`, as [`evaluation_job_type_of`] reads it off
    /// [`Self::evaluation_config`].
    pub job_type: &'static str,
    /// `EvaluationJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`EVALUATION_JOB_STATUS_COMPLETED`], or
    /// `None` once the run has ended or a caller has stopped or deleted the job.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to [`EVALUATION_JOB_STATUS_STOPPED`], or
    /// `None` while no stop is in flight.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub stopped_due_at: Option<SystemTime>,
    /// When this job, which a batch delete accepted, is owed its removal from the store —
    /// and `None` while no delete has asked for one.
    ///
    /// The due INSTANT, not a timer, for the same reason [`Self::stopped_due_at`] is one.
    pub deleted_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed, which is the creation instant until something changes
    /// it.
    pub last_modified_at: SystemTime,
}

impl EvaluationJobRecord {
    /// Whether this job is one a stop may be asked for: it is still running.
    ///
    /// "Stops an evaluation job that is current being created or running"
    /// (`smithy_query(query="operation",operation="StopEvaluationJob")`), so a job in any
    /// other status has nothing left to stop.
    #[must_use]
    pub fn is_stoppable(&self) -> bool {
        self.status == EVALUATION_JOB_STATUS_IN_PROGRESS
    }

    /// Move this job to [`EVALUATION_JOB_STATUS_STOPPING`] and record when the stop is owed
    /// its move to [`EVALUATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopEvaluationJob` — not a clock transition:
    /// [`Self::settle`] is what materialises the one the clock owes. The completion the
    /// clock owed a running job is no longer owed once the run is being wound down, so it
    /// is dropped here.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = EVALUATION_JOB_STATUS_STOPPING;
        self.completed_due_at = None;
        self.stopped_due_at = Some(now + EVALUATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
    }

    /// Whether this job is one a batch delete may accept: its status is terminal.
    ///
    /// "An evaluation job can only be deleted if it has following status FAILED,
    /// COMPLETED, and STOPPED"
    /// (`smithy_query(query="operation",operation="BatchDeleteEvaluationJob")`), so a job
    /// in any other status — one still running, one winding down, one already being
    /// deleted — is not deletable yet.
    #[must_use]
    pub fn is_deletable(&self) -> bool {
        EVALUATION_JOB_DELETABLE_STATUSES.contains(&self.status)
    }

    /// Move this job to [`EVALUATION_JOB_STATUS_DELETING`] and record when its removal from
    /// the store is owed.
    ///
    /// The caller's own write, asked for by `BatchDeleteEvaluationJob` — not a clock
    /// transition: [`BedrockState::evaluation_job`] is what materialises the removal the
    /// clock owes.
    pub fn delete(&mut self, now: SystemTime) {
        self.status = EVALUATION_JOB_STATUS_DELETING;
        self.deleted_due_at = Some(now + EVALUATION_JOB_DELETED_AFTER);
        self.last_modified_at = now;
    }

    /// Whether this job's removal from the store is one the clock has already made due at
    /// `now`.
    #[must_use]
    pub fn is_removal_due(&self, now: SystemTime) -> bool {
        self.deleted_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the status move the clock has already made due at `now`: a running job's
    /// move to [`EVALUATION_JOB_STATUS_COMPLETED`], or a stopping job's to
    /// [`EVALUATION_JOB_STATUS_STOPPED`].
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its last modification. At most one of the two is pending,
    /// since a stop drops the completion the clock owed the run it wound down.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.completed_due_at.filter(|due| now >= *due) {
            self.status = EVALUATION_JOB_STATUS_COMPLETED;
            self.completed_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = EVALUATION_JOB_STATUS_STOPPED;
            self.stopped_due_at = None;
            self.last_modified_at = due;
        }
    }
}

/// One stored advanced prompt optimization job: the identity
/// `CreateAdvancedPromptOptimizationJob` minted, the configuration that request carried,
/// the status it holds now and the instants a read reports.
///
/// `status` is `&'static str` for the same reason [`EvaluationJobRecord::status`] is: it is
/// a value this emulator derives rather than anything a request names.
#[derive(Debug, Clone)]
pub struct AdvancedPromptOptimizationJobRecord {
    /// The `AdvancedPromptOptimizationJobArn` this job is addressed by, minted by
    /// [`advanced_prompt_optimization_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so a bare-id lookup is per region without re-parsing an ARN, exactly as
    /// [`EvaluationJobRecord::region`] is.
    pub region: String,
    /// `jobDescription`, as the create request carried it (absent when it carried none).
    pub description: Option<String>,
    /// `inputConfig` — the S3 location of the JSONL input the job optimizes over.
    pub input_config: AdvancedPromptOptimizationInputConfig,
    /// `outputConfig` — the S3 prefix the optimization results are written under.
    pub output_config: AdvancedPromptOptimizationOutputConfig,
    /// `encryptionKeyArn`, as the create request carried it (absent when it carried none,
    /// in which case the output is encrypted with an Amazon-owned key and the read reports
    /// no key). Stored VERBATIM: the member is a `KmsKeyArn` in both the request and the
    /// response, so the ARN the request gave is the ARN the read reports.
    pub encryption_key_arn: Option<String>,
    /// `modelConfigurations` — the target models the job optimizes the prompts for, in the
    /// order the create request listed them.
    pub model_configurations: Vec<ModelConfiguration>,
    /// `tags` — the tags the create request attached.
    ///
    /// Kept with the record because it is part of the request that minted it; this
    /// resource's read operations report no tag member, so nothing reads them back, exactly
    /// as [`ProvisionedModelThroughputRecord::tags`] are kept.
    pub tags: Vec<Tag>,
    /// `AdvancedPromptOptimizationJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`], or `None` while no stop is in
    /// flight.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`EvaluationJobRecord::stopped_due_at`] is one.
    pub stopped_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed, which is the creation instant until something changes it.
    pub last_modified_at: SystemTime,
}

impl AdvancedPromptOptimizationJobRecord {
    /// Whether this job is one a stop may be asked for: it is still in progress.
    ///
    /// "Stops an advanced prompt optimization job that is in progress"
    /// (`smithy_query(query="doc",shape="StopAdvancedPromptOptimizationJob")`), so a job in
    /// any other status has nothing left to stop.
    #[must_use]
    pub fn is_in_progress(&self) -> bool {
        self.status == ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_IN_PROGRESS
    }

    /// Whether a stop has already taken this job over: it is winding down, or has wound
    /// down.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.status == ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING
            || self.status == ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED
    }

    /// Move this job to [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING`] and record
    /// when the stop is owed its move to
    /// [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopAdvancedPromptOptimizationJob` — not a
    /// clock transition: [`Self::settle`] is what materialises the one the clock owes.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPING;
        self.stopped_due_at = Some(now + ADVANCED_PROMPT_OPTIMIZATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
    }

    /// Materialise the move to [`ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED`] the
    /// clock has already made due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the job reports as its last modification.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = ADVANCED_PROMPT_OPTIMIZATION_JOB_STATUS_STOPPED;
            self.stopped_due_at = None;
            self.last_modified_at = due;
        }
    }
}

/// One stored Provisioned Throughput: the identity `CreateProvisionedModelThroughput`
/// minted, the model units and commitment that request bought, the model it is associated
/// with, and the lifecycle status a read reports.
///
/// `status` is `&'static str` for the same reason [`ModelImportJobRecord::status`] is: it
/// is one of the two values this emulator mints
/// ([`PROVISIONED_MODEL_STATUS_CREATING`], [`PROVISIONED_MODEL_STATUS_IN_SERVICE`])
/// rather than anything a request chooses.
#[derive(Debug, Clone)]
pub struct ProvisionedModelThroughputRecord {
    /// The `ProvisionedModelArn` this Provisioned Throughput is addressed by, minted by
    /// [`provisioned_model_arn`].
    pub arn: String,
    /// `provisionedModelName`, as the create request spelled it. The other spelling of
    /// `ProvisionedModelId`, so it is also what makes the name unique within the account
    /// and region.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN, exactly as
    /// [`EvaluationJobRecord::region`] is.
    pub region: String,
    /// `modelArn` — the model this Provisioned Throughput is associated with NOW.
    pub model_arn: String,
    /// `desiredModelArn` — the model it was last requested to be associated with. Equal to
    /// [`Self::model_arn`] until an association change is in flight.
    pub desired_model_arn: String,
    /// `foundationModelArn` — the base model the Provisioned Throughput was created for,
    /// as [`foundation_model_arn`] spells it.
    pub foundation_model_arn: String,
    /// `modelUnits` — the model units allocated now.
    pub model_units: i32,
    /// `desiredModelUnits` — the model units last requested. Equal to [`Self::model_units`]
    /// until an allocation change is in flight.
    pub desired_model_units: i32,
    /// `ProvisionedModelStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`PROVISIONED_MODEL_STATUS_IN_SERVICE`],
    /// or `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub in_service_due_at: Option<SystemTime>,
    /// `commitmentDuration`, as the create request carried it (absent for a no-commitment
    /// Provisioned Throughput).
    pub commitment_duration: Option<String>,
    /// `commitmentExpirationTime` — when the commitment term ends: the creation instant
    /// plus the term [`commitment_term`] reads off [`Self::commitment_duration`], and
    /// `None` when there is no commitment.
    pub commitment_expiration_time: Option<SystemTime>,
    /// `tags` — the tags the create request attached.
    ///
    /// Kept with the record because it is part of the request that minted it; the
    /// Provisioned Throughput read operations report no tag member, so nothing reads them
    /// back until `ListTagsForResource` is implemented.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// When the Provisioned Throughput last changed: the creation instant until something
    /// changes it, and the instant a settle was owed once the status has moved.
    pub last_modified_at: SystemTime,
}

impl ProvisionedModelThroughputRecord {
    /// Materialise the move to [`PROVISIONED_MODEL_STATUS_IN_SERVICE`] the clock has
    /// already made due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the record reports as its last modification.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.in_service_due_at.filter(|due| now >= *due) {
            self.status = PROVISIONED_MODEL_STATUS_IN_SERVICE;
            self.in_service_due_at = None;
            self.last_modified_at = due;
        }
    }
}

/// One custom model deployment: the on-demand inference deployment
/// `CreateCustomModelDeployment` mints over a custom model and
/// `GetCustomModelDeployment` reads back.
#[derive(Debug, Clone)]
pub struct CustomModelDeploymentRecord {
    /// `customModelDeploymentArn` — the ARN this deployment is addressed by, and the key
    /// it is stored under ([`custom_model_deployment_arn`]).
    pub arn: String,
    /// `modelDeploymentName` — the name the create request gave it, unique within the
    /// account and region, and the OTHER spelling of a
    /// `CustomModelDeploymentIdentifier`.
    pub name: String,
    /// The region the create request addressed: a deployment belongs to one region, as the
    /// name's uniqueness does.
    pub region: String,
    /// `modelArn` — the custom model this deployment serves, as the create request carried
    /// it.
    pub model_arn: String,
    /// `description` — the create request's description, absent when it carried none.
    pub description: Option<String>,
    /// `tags` — the tags the create request attached.
    ///
    /// The ONE copy of this deployment's tags: the list the create filled is the list
    /// `ops/tag_resource.rs` rewrites and `ops/list_tags_for_resource.rs` reports, reached
    /// through [`ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::tagged_resource`].
    pub tags: Vec<Tag>,
    /// `status` — a [`CustomModelDeploymentStatus`](CUSTOM_MODEL_DEPLOYMENT_STATUS_CREATING).
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`], or `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub active_due_at: Option<SystemTime>,
    /// `createdAt` — when the create call accepted this deployment.
    pub created_at: SystemTime,
    /// `lastUpdatedAt` — when the deployment last changed, and `None` while nothing has
    /// changed it since it was created (the member is optional in the model).
    pub last_updated_at: Option<SystemTime>,
}

impl CustomModelDeploymentRecord {
    /// Materialise the move to [`CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE`] the clock has
    /// already made due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called, which is why that instant — not `now` — is the
    /// one the record reports as its last update.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.active_due_at.filter(|due| now >= *due) {
            self.status = CUSTOM_MODEL_DEPLOYMENT_STATUS_ACTIVE;
            self.active_due_at = None;
            self.last_updated_at = Some(due);
        }
    }
}

/// One stored custom model: the identity `CreateCustomModel` minted, the data source and
/// key that request carried, the base model it reports, and the status a read reports.
///
/// `status` and `customization_type` are `&'static str` for the same reason
/// [`ModelImportJobRecord::status`] is: both are values this emulator derives
/// ([`CUSTOM_MODEL_STATUS_CREATING`], [`CUSTOMIZATION_TYPE_IMPORTED`]) rather than
/// anything a request names.
#[derive(Debug, Clone)]
pub struct CustomModelRecord {
    /// The `CustomModelArn` this model is addressed by, minted by [`custom_model_arn`].
    pub arn: String,
    /// `modelName`, as the create request spelled it \u2014 the other spelling a
    /// `ModelIdentifier` may carry.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index is per region without re-parsing an ARN, exactly as
    /// [`ProvisionedModelThroughputRecord::region`] is.
    pub region: String,
    /// `baseModelArn` \u2014 the model this one was customized from.
    ///
    /// For a model created from ARTIFACTS this is the model's OWN ARN: the create request
    /// names no foundation model, and `ModelArn` admits a `custom-model` ARN here, so the
    /// model is reported as its own base rather than attributed to a foundation model
    /// nothing in the request named.
    pub base_model_arn: String,
    /// `baseModelName` \u2014 the base model's name, which for a model created from artifacts
    /// is the base segment its own ARN carries ([`CUSTOM_MODEL_IMPORTED_BASE`]).
    pub base_model_name: String,
    /// The `FoundationModelArn` of the foundation model this custom model descends from,
    /// or `None` when it descends from none \u2014 which is what `ListCustomModels`'
    /// `foundationModelArnEquals` filters on, as distinct from `baseModelArnEquals`.
    pub foundation_model_arn: Option<String>,
    /// `jobArn` — the `ModelCustomizationJobArn` of the customization job that PRODUCED
    /// this model, or `None` for a model that no job produced.
    ///
    /// "For models that you create with the CreateCustomModel API operation, this is NULL"
    /// (`smithy_query(query="operation",operation="GetCustomModel")`), so this is also what
    /// says which of the members a read reports are the JOB's: the job this names is where
    /// the hyperparameters, the datasets and the output location live, so there is only ever
    /// one copy of them.
    pub job_arn: Option<String>,
    /// `CustomizationType`.
    pub customization_type: &'static str,
    /// `modelSourceConfig`, as the create request carried it (absent when the request
    /// named the model package source instead).
    pub model_source_config: Option<ModelDataSource>,
    /// `customModelDataSource`, as the create request carried it (absent when the request
    /// named the Amazon S3 source instead).
    pub custom_model_data_source: Option<CustomModelDataSource>,
    /// `modelKmsKeyArn`, as the create request carried it \u2014 already a `KmsKeyArn` there,
    /// so it is stored and reported verbatim.
    pub model_kms_key_arn: Option<String>,
    /// `roleArn` \u2014 the service role Amazon Bedrock assumes to read the artifacts (absent
    /// when the create carried none).
    ///
    /// Kept with the record because it is part of the request that minted it; no read
    /// operation of this resource reports a role member.
    pub role_arn: Option<String>,
    /// `modelTags` \u2014 the tags the create request attached to the model.
    ///
    /// Kept for the reason [`ProvisionedModelThroughputRecord::tags`] is: it is part of
    /// the request that minted the record, and nothing reads it back until
    /// `ListTagsForResource` is implemented.
    pub tags: Vec<Tag>,
    /// `ModelStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`CUSTOM_MODEL_STATUS_ACTIVE`], or `None`
    /// once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`MarketplaceModelEndpointRecord::in_service_due_at`] is one.
    pub active_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
}

impl CustomModelRecord {
    /// Materialise the move to [`CUSTOM_MODEL_STATUS_ACTIVE`] the clock has already made
    /// due at `now`.
    ///
    /// The only write a reporting operation on this resource makes: it was owed at the due
    /// instant even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.active_due_at.is_some_and(|due| now >= due) {
            self.status = CUSTOM_MODEL_STATUS_ACTIVE;
            self.active_due_at = None;
        }
    }
}

/// One stored model copy job: the identity `CreateModelCopyJob` minted, the source model
/// the request named, the copied model the job produces, and the job's own lifecycle
/// status.
///
/// `status` is `&'static str` for the same reason [`CustomModelRecord::status`] is: the
/// statuses are the ones this emulator's lifecycle passes through rather than anything a
/// request chooses.
#[derive(Debug, Clone)]
pub struct ModelCopyJobRecord {
    /// The `ModelCopyJobArn` this job is addressed by, minted by [`model_copy_job_arn`].
    pub arn: String,
    /// The region the create addressed — the region the copy is made INTO, which
    /// [`Self::arn`] also carries.
    pub region: String,
    /// `sourceModelArn`, as the create request spelled it. The model being copied lives in
    /// the region its own ARN names, which is how this operation copies across regions.
    pub source_model_arn: String,
    /// `sourceModelName` — the name of the custom model being copied, as that model
    /// carries it.
    pub source_model_name: Option<String>,
    /// `sourceAccountId` — the account the model being copied originated from, read off the
    /// source ARN.
    pub source_account_id: String,
    /// `targetModelArn` — the `CustomModelArn` the copied model is minted under, decided
    /// when the job is accepted so the job reports the model it will produce from the
    /// start.
    pub target_model_arn: String,
    /// `targetModelName`, as the create request spelled it.
    pub target_model_name: String,
    /// `targetModelKmsKeyArn` — the key the copy is encrypted with (absent when the create
    /// named none).
    pub target_model_kms_key_arn: Option<String>,
    /// `targetModelTags` — the tags the create attached to the copied model.
    pub target_model_tags: Vec<Tag>,
    /// `ModelCopyJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to [`MODEL_COPY_JOB_STATUS_COMPLETED`], or
    /// `None` once it is terminal.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`CustomModelRecord::active_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
}

/// One stored model customization job: the identity `CreateModelCustomizationJob` minted,
/// the request that minted it, the custom model it is customizing, and the job's own
/// lifecycle status.
///
/// `status` and `customization_type` are `&'static str` for the same reason
/// [`ModelImportJobRecord::status`] is: the statuses are the ones this emulator's lifecycle
/// passes through, and the customization type is resolved against the modelled enum
/// ([`customization_type_of`]) rather than kept as whatever text arrived.
#[derive(Debug, Clone)]
pub struct ModelCustomizationJobRecord {
    /// The `ModelCustomizationJobArn` this job is addressed by, minted by
    /// [`model_customization_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it — the other spelling a
    /// `ModelCustomizationJobIdentifier` may carry.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it
    /// so the name index and the uniqueness checks are per region without re-parsing an
    /// ARN, exactly as [`ModelImportJobRecord::region`] is.
    pub region: String,
    /// `clientRequestToken`, as the create request carried it — a member the read reports
    /// back.
    pub client_request_token: Option<String>,
    /// `roleArn` — the service role Amazon Bedrock assumes to read the training data and
    /// write the model artifacts.
    pub role_arn: String,
    /// `customModelName` — the name of the model this job is customizing, which the read
    /// reports as its `outputModelName`.
    pub output_model_name: String,
    /// The `CustomModelArn` of the model this job is customizing, minted by
    /// [`custom_model_arn`] when the job is accepted: it is the job's target, so it is
    /// fixed from that call onwards, exactly as [`ModelImportJobRecord::imported_model_arn`]
    /// is.
    pub output_model_arn: String,
    /// `baseModelArn` — the `FoundationModelArn` of the base model being customized, as
    /// [`foundation_model_arn`] spells it for the catalog row the request's
    /// `baseModelIdentifier` resolved to.
    pub base_model_arn: String,
    /// `CustomizationType` — the customization this job runs.
    pub customization_type: &'static str,
    /// The `outputModelKmsKeyArn` a read reports for the request's `customModelKmsKeyId`,
    /// as [`kms_key_arn`] completes it.
    pub output_model_kms_key_arn: Option<String>,
    /// `hyperParameters`, as the create request carried them (absent when it carried none).
    pub hyper_parameters: Option<HashMap<String, String>>,
    /// `trainingDataConfig` — where the training data is read from.
    pub training_data_config: TrainingDataConfig,
    /// `validationDataConfig` — the validators the job runs.
    ///
    /// Stored as the shape rather than as an `Option` of it: the member is optional in the
    /// request and REQUIRED in the read
    /// (`smithy_query(query="operation",operation="GetModelCustomizationJob")`), and its
    /// own `validators` list admits zero entries, so a create that named no validation data
    /// is the job with no validators.
    pub validation_data_config: ValidationDataConfig,
    /// `outputDataConfig` — where the job writes its output.
    pub output_data_config: OutputDataConfig,
    /// `vpcConfig`, as the create request carried it (absent when it carried none).
    pub vpc_config: Option<VpcConfig>,
    /// `customizationConfig`, as the create request carried it (absent when it carried
    /// none).
    pub customization_config: Option<CustomizationConfig>,
    /// `jobTags` — the tags attached to the job itself.
    ///
    /// Kept for the reason [`ProvisionedModelThroughputRecord::tags`] is: it is part of the
    /// request that minted the record, and the job's own read reports no tag member.
    pub job_tags: Vec<Tag>,
    /// `customModelTags` — the tags attached to the model this job is customizing, which
    /// become that model's tags when the job completes.
    pub custom_model_tags: Vec<Tag>,
    /// `ModelCustomizationJobStatus`.
    pub status: &'static str,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED`], or `None` once the job is terminal or
    /// a stop has been asked for.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED`], or `None` while no stop is in flight.
    pub stopped_due_at: Option<SystemTime>,
    pub created_at: SystemTime,
    /// When the job last changed: the creation instant until something changes it, and the
    /// instant a transition was owed once the status has moved.
    pub last_modified_at: SystemTime,
    /// When the job reached a terminal status, or `None` while it is still running.
    pub ended_at: Option<SystemTime>,
}

impl ModelCustomizationJobRecord {
    /// Whether this job is still running, so a stop has something to wind down.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.status == MODEL_CUSTOMIZATION_JOB_STATUS_IN_PROGRESS
    }

    /// Whether a stop has already taken this job over: it is winding down, or it has
    /// wound down.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.status == MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING
            || self.status == MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED
    }

    /// Move this job to [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING`] and record when the
    /// stop is owed its move to [`MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopModelCustomizationJob` — not a clock
    /// transition. The training it was running is being wound down, so the job is owed no
    /// completion any more and mints no custom model.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING;
        self.stopped_due_at = Some(now + MODEL_CUSTOMIZATION_JOB_STOPPED_AFTER);
        self.completed_due_at = None;
        self.last_modified_at = now;
    }
}

/// `ModelInvocationJobStatus` — a batch inference job that has been submitted: "Submitted
/// – This job has been submitted to a queue for validation"
/// (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`). The
/// status every job this emulator mints carries from the call that submits it until the
/// clock moves it on.
pub const MODEL_INVOCATION_JOB_STATUS_SUBMITTED: &str = "Submitted";

/// `ModelInvocationJobStatus` — a batch inference job being checked over before it is
/// queued to run: "Validating – This job is being validated for the requirements described
/// in Format and upload your batch inference data".
pub const MODEL_INVOCATION_JOB_STATUS_VALIDATING: &str = "Validating";

/// `ModelInvocationJobStatus` — a validated batch inference job waiting its turn:
/// "Scheduled – This job has been validated and is now in a queue. The job will
/// automatically start when it reaches its turn".
pub const MODEL_INVOCATION_JOB_STATUS_SCHEDULED: &str = "Scheduled";

/// `ModelInvocationJobStatus` — a batch inference job that has begun: "InProgress – This
/// job has begun. You can start viewing the results in the output S3 location".
pub const MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job that ran to
/// the end of its input: "Completed – This job has successfully completed".
pub const MODEL_INVOCATION_JOB_STATUS_COMPLETED: &str = "Completed";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job some but not
/// all of whose records were processed: "PartiallyCompleted – This job has partially
/// completed. Not all of your records could be processed in time. View the output files in
/// the output S3 location".
pub const MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED: &str = "PartiallyCompleted";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job that could not
/// be run: "Failed – This job has failed. Check the failure message for any further
/// details". The failure message is [`ModelInvocationJobRecord::message`].
pub const MODEL_INVOCATION_JOB_STATUS_FAILED: &str = "Failed";

/// `ModelInvocationType` — the Converse endpoint, one of the two values the modelled enum
/// carries (`crate::ops::types::MODEL_INVOCATION_TYPE_VALUES`). Named because the job's
/// work step invokes each record through the endpoint the submission chose.
pub const MODEL_INVOCATION_TYPE_CONVERSE: &str = "Converse";

/// `ModelInvocationJobStatus` — a batch inference job a user asked to stop, while that
/// stop is still being carried out: "Stopping – This job is being stopped by a user".
pub const MODEL_INVOCATION_JOB_STATUS_STOPPING: &str = "Stopping";

/// `ModelInvocationJobStatus` — the terminal status of a batch inference job a user
/// stopped: "Stopped – This job was stopped by a user".
pub const MODEL_INVOCATION_JOB_STATUS_STOPPED: &str = "Stopped";

/// The `ModelInvocationJobStatus` values a batch inference job can still be stopped from:
/// a job that has been submitted, is being validated, is queued or is running has work a
/// stop winds down (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`).
/// Every other value of the enum is either terminal or a stop already in flight, which is
/// why a stop of one of those changes nothing.
pub const MODEL_INVOCATION_JOB_STOPPABLE_STATUSES: &[&str] = &[
    MODEL_INVOCATION_JOB_STATUS_SUBMITTED,
    MODEL_INVOCATION_JOB_STATUS_VALIDATING,
    MODEL_INVOCATION_JOB_STATUS_SCHEDULED,
    MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS,
];

/// How long after `StopModelInvocationJob` accepted the stop a batch inference job is owed
/// its move from [`MODEL_INVOCATION_JOB_STATUS_STOPPING`] to
/// [`MODEL_INVOCATION_JOB_STATUS_STOPPED`]. Chosen for the reason
/// [`MODEL_INVOCATION_JOB_BEGINS_AFTER`] is, and non-zero so a caller reading the job
/// straight after the stop still sees the status the stop recorded.
pub const MODEL_INVOCATION_JOB_STOPPED_AFTER: Duration = Duration::from_millis(500);

/// How long after it was submitted a batch inference job is owed its move from
/// [`MODEL_INVOCATION_JOB_STATUS_SUBMITTED`] to
/// [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`].
///
/// The emulator's own span, chosen like [`IMPORT_JOB_COMPLETE_AFTER`] rather than read off
/// any published figure: the queueing a real submission waits through is not something this
/// emulator performs, and the span is non-zero so a caller reading the job straight after
/// the create still sees the status the create answered with.
pub const MODEL_INVOCATION_JOB_VALIDATES_AFTER: Duration = Duration::from_millis(500);

/// How long after its validation began a batch inference job is owed its move from
/// [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`] to
/// [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`]. Chosen, and non-zero, for the reason
/// [`MODEL_INVOCATION_JOB_VALIDATES_AFTER`] is: every pre-start status the model documents
/// is one a caller can read the job in rather than one it is carried past.
pub const MODEL_INVOCATION_JOB_SCHEDULES_AFTER: Duration = Duration::from_millis(500);

/// How long after it was queued for its turn a batch inference job is owed its move from
/// [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`] to
/// [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`]. Chosen for the same reason
/// [`MODEL_INVOCATION_JOB_VALIDATES_AFTER`] is.
pub const MODEL_INVOCATION_JOB_BEGINS_AFTER: Duration = Duration::from_millis(500);

/// How long a running batch inference job dwells in
/// [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`] before it is owed its move to
/// [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`].
///
/// CHOSEN, not documented: a real submission runs for as long as its records and the queue
/// it is in take, which is no published figure, so this is the emulator's own span. It is
/// counted from the instant the job was owed its start
/// ([`ModelInvocationJobClock::armed_at`]), so the whole progression stands on the
/// SUBMISSION's own timeline and the elapsed time since the submission — not the number of
/// times anybody read the job — is what carries it to its terminal status.
///
/// It is LONGER than the three spans before it, because this is the one status of the
/// progression a caller is documented to act on while it holds: "InProgress – this job has
/// begun. You can start viewing the results in the output S3 location", and it is the last
/// status a stop still has work to wind down from
/// ([`MODEL_INVOCATION_JOB_STOPPABLE_STATUSES`]). A client of a job the service documents as
/// long-running polls it at intervals of seconds to tens of seconds, so a running window
/// shorter than that would be one no such client ever reads the job in — and then the
/// progress it watches and the stop it issues on a running job would both be unreachable.
/// The window is of that order and no larger: the terminal status, the record counts and the
/// results under the output prefix are still reached inside one short polling pass rather
/// than after a wall-clock wait of the size a caller budgets for the whole interaction.
///
/// The WORK the span stands over is performed, not skipped
/// ([`model_invocation_run::run_batch_inference`] reads the input and invokes the model once
/// per record), and the span does not scale with the input: it is materialised from the
/// stored due instant on the next call that touches the record rather than waited on, so no
/// handler sleeps.
pub const MODEL_INVOCATION_JOB_COMPLETE_AFTER: Duration = Duration::from_secs(12);

/// The EARLIEST instants a batch inference job submitted at a given moment is owed each of
/// its status transitions at: the whole progression
/// `Submitted -> Validating -> Scheduled -> InProgress -> Completed`, each stage standing its
/// own span after the one before it.
///
/// The submission arms them all and nothing re-counts them afterwards
/// ([`ModelInvocationJobRecord::settle`]), so these are the instants every caller reads the
/// job's stages at, whether it polls once or a hundred times: a stage whose instant has
/// elapsed is behind the job for everybody.
#[derive(Debug, Clone, Copy)]
pub struct ModelInvocationJobClock {
    /// When [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`] is owed.
    pub validating_at: SystemTime,
    /// When [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`] is owed.
    pub scheduled_at: SystemTime,
    /// When [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`] is owed.
    pub in_progress_at: SystemTime,
    /// When [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`] is owed.
    pub completed_at: SystemTime,
}

impl ModelInvocationJobClock {
    /// The clock of a job submitted at `submit_time`: each transition stands its own span
    /// after the transition before it.
    ///
    /// The instants are strictly increasing, so the progression is walked in order whatever
    /// instant a caller observes it at: a job read exactly as one stage falls due holds that
    /// stage, and one first read long after the submission is carried through every stage
    /// whose instant has already passed, to the status the elapsed time owes it.
    #[must_use]
    pub fn armed_at(submit_time: SystemTime) -> Self {
        let validating_at = submit_time + MODEL_INVOCATION_JOB_VALIDATES_AFTER;
        let scheduled_at = validating_at + MODEL_INVOCATION_JOB_SCHEDULES_AFTER;
        let in_progress_at = scheduled_at + MODEL_INVOCATION_JOB_BEGINS_AFTER;
        let completed_at = in_progress_at + MODEL_INVOCATION_JOB_COMPLETE_AFTER;
        Self {
            validating_at,
            scheduled_at,
            in_progress_at,
            completed_at,
        }
    }
}

/// The seconds in the hour a `timeoutDurationInHours` counts, so the instant a job times
/// out is computed on one scale wherever it is needed.
pub const SECONDS_PER_HOUR: u64 = 60 * 60;

/// The most batch inference jobs this account may hold at once, across every region it has
/// submitted one in, past which a create is refused with `ServiceQuotaExceededException`.
///
/// The emulator's own bound rather than a published quota, for the reason
/// [`MAX_MODEL_CUSTOMIZATION_JOBS_PER_ACCOUNT`] is one: without a server-side cap a caller
/// could grow this part of the store with nothing but further submissions.
pub const MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT: usize = 100;

/// One batch inference job, as `CreateModelInvocationJob` submitted it and
/// `GetModelInvocationJob` and `ListModelInvocationJobs` report it.
///
/// `status` and `model_invocation_type` are `&'static str` for the reason
/// [`ModelCustomizationJobRecord::status`] is: the statuses are the ones this emulator's
/// lifecycle passes through, and the invocation type is resolved against the modelled enum
/// ([`model_invocation_type_of`]) rather than kept as whatever text arrived.
#[derive(Debug, Clone)]
pub struct ModelInvocationJobRecord {
    /// The `ModelInvocationJobArn` this job is addressed by, minted by
    /// [`model_invocation_job_arn`].
    pub arn: String,
    /// `jobName`, as the create request spelled it. NOT an identifier: a
    /// `ModelInvocationJobIdentifier` is the job ARN or the minted job id alone
    /// (`smithy_query(query="operation",operation="GetModelInvocationJob")`), so a name is
    /// reported and filtered on rather than resolved through.
    pub name: String,
    /// The region the create addressed, which [`Self::arn`] also carries. Kept beside it so
    /// a listing is per region without re-parsing an ARN, exactly as
    /// [`ModelCustomizationJobRecord::region`] is.
    pub region: String,
    /// `clientRequestToken`, as the create request carried it — a member both reads report
    /// back.
    pub client_request_token: Option<String>,
    /// `roleArn` — the service role Amazon Bedrock assumes to read the input and write the
    /// output of this job.
    pub role_arn: String,
    /// `modelId`, as the create request spelled it: the identifier admits a bare model id,
    /// a foundation model ARN, a custom model ARN and an inference profile ARN, and the
    /// reads report the spelling the submission carried.
    pub model_id: String,
    /// `inputDataConfig` — where the job reads its records from.
    pub input_data_config: ModelInvocationJobInputDataConfig,
    /// `outputDataConfig` — where the job writes its records to.
    pub output_data_config: ModelInvocationJobOutputDataConfig,
    /// `vpcConfig`, as the create request carried it (absent when it carried none).
    pub vpc_config: Option<VpcConfig>,
    /// `timeoutDurationInHours`, as the create request carried it (absent when it carried
    /// none, which is a job with no timeout to report).
    pub timeout_duration_in_hours: Option<i32>,
    /// `jobExpirationTime` — "the time at which the batch inference job times or timed
    /// out": [`Self::submit_time`] plus [`Self::timeout_duration_in_hours`], and `None` for
    /// a job the request set no timeout for.
    pub job_expiration_time: Option<SystemTime>,
    /// `ModelInvocationType` — the invocation endpoint this job runs against.
    pub model_invocation_type: &'static str,
    /// `tags` — the tags the submission attached to the job.
    ///
    /// Kept for the reason [`ProvisionedModelThroughputRecord::tags`] is: it is part of the
    /// request that minted the record, and the job's own reads report no tag member.
    pub tags: Vec<Tag>,
    /// `ModelInvocationJobStatus`.
    pub status: &'static str,
    /// The terminal `ModelInvocationJobStatus` this job's [`Self::completed_due_at`] moves
    /// it to: [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`] for a job whose every record was
    /// processed, [`MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED`] for one some of whose
    /// records could not be, and [`MODEL_INVOCATION_JOB_STATUS_FAILED`] for one that could
    /// not be run at all.
    ///
    /// Decided by the WORK, not by the clock: the submission's validation of the input
    /// ([`model_invocation_run::read_input_records`]) and the run over its records
    /// ([`model_invocation_run::run_batch_inference`]) are what set it, so the status a job
    /// ends in reports what happened to its records.
    pub terminal_status: &'static str,
    /// `message` — "if the batch inference job failed, this field contains a message
    /// describing why the job failed", and `None` for a job that has not failed.
    pub message: Option<String>,
    /// `totalRecordCount` — "the total number of records in the batch inference job":
    /// the records read out of the input object this job was pointed at.
    ///
    /// `None` only while nothing is known about that object: an emulator assembled with no
    /// object store to ask establishes neither its records nor its absence
    /// ([`model_invocation_run::InputVerdict::Unknown`]), and a count is reported only
    /// where one was counted.
    pub total_record_count: Option<i64>,
    /// `processedRecordCount` — "the number of records that have been processed in the
    /// batch inference job": [`Self::success_record_count`] and
    /// [`Self::error_record_count`] together, zero until the job's run materialises.
    pub processed_record_count: i64,
    /// `successRecordCount` — "the number of records that were successfully processed in
    /// the batch inference job": the records the model answered for.
    pub success_record_count: i64,
    /// `errorRecordCount` — "the number of records that failed to process in the batch
    /// inference job": the records the model could not be invoked for.
    pub error_record_count: i64,
    /// Whether this job's run — reading its records, invoking the model over each of them
    /// and storing the results under its output prefix — is still owed.
    ///
    /// The run is owed from the submission that validated an input, falls due with the
    /// status that says the job has begun, and is taken ONCE ([`Self::due_run`]): a run the
    /// object store did not take is given back, so a later call takes it again, and no
    /// later call runs a job whose results are already stored.
    pub run_owed: bool,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_VALIDATING`], or `None` once that move has been
    /// materialised.
    ///
    /// The due INSTANT, not a timer, for the same reason
    /// [`ModelImportJobRecord::completed_due_at`] is one. It and the three below are the
    /// job's whole clock, armed by the submission ([`ModelInvocationJobClock::armed_at`]) and
    /// only ever spent — never re-counted — by the calls that observe the job
    /// ([`ModelInvocationJobRecord::settle`]), so the progression a caller reads is the one
    /// the time since the submission owes it.
    pub validating_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_SCHEDULED`], or `None` once that move has been
    /// materialised.
    pub scheduled_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS`], or `None` once the job has begun.
    pub in_progress_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_COMPLETED`], or `None` once the job has reached a
    /// terminal status.
    pub completed_due_at: Option<SystemTime>,
    /// When [`Self::status`] is owed its move to
    /// [`MODEL_INVOCATION_JOB_STATUS_STOPPED`], or `None` while no stop is in flight.
    pub stopped_due_at: Option<SystemTime>,
    /// `submitTime` — when the job was submitted.
    pub submit_time: SystemTime,
    /// `lastModifiedTime`: the submission instant until something changes the job, and the
    /// instant a transition was owed once the status has moved.
    pub last_modified_at: SystemTime,
    /// `endTime` — when the job reached a terminal status, or `None` while it has not.
    pub ended_at: Option<SystemTime>,
}

impl ModelInvocationJobRecord {
    /// Whether a stop has work to wind down on this job: its status is one a job that has
    /// not finished carries ([`MODEL_INVOCATION_JOB_STOPPABLE_STATUSES`]).
    #[must_use]
    pub fn is_stoppable(&self) -> bool {
        MODEL_INVOCATION_JOB_STOPPABLE_STATUSES.contains(&self.status)
    }

    /// Move this job to [`MODEL_INVOCATION_JOB_STATUS_STOPPING`] and record when the stop
    /// is owed its move to [`MODEL_INVOCATION_JOB_STATUS_STOPPED`].
    ///
    /// The caller's own write, asked for by `StopModelInvocationJob` — not a clock
    /// transition. The inference it was queued for or running is being wound down, so none
    /// of the transitions the submission armed ([`ModelInvocationJobClock::armed_at`]) is
    /// owed any more.
    pub fn stop(&mut self, now: SystemTime) {
        self.status = MODEL_INVOCATION_JOB_STATUS_STOPPING;
        self.validating_due_at = None;
        self.scheduled_due_at = None;
        self.in_progress_due_at = None;
        self.completed_due_at = None;
        self.stopped_due_at = Some(now + MODEL_INVOCATION_JOB_STOPPED_AFTER);
        self.last_modified_at = now;
        // The run it was queued for is being wound down, so the records it would have
        // processed are not processed and nothing is written for them.
        self.run_owed = false;
    }

    /// Whether the job has BEGUN: its status is one the model documents the results of the
    /// run under — "InProgress – this job has begun. You can start viewing the results in
    /// the output S3 location", and the terminal statuses that say to view those files.
    #[must_use]
    pub fn has_begun(&self) -> bool {
        matches!(
            self.status,
            MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS
                | MODEL_INVOCATION_JOB_STATUS_COMPLETED
                | MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED
        )
    }

    /// The run this job still owes, taken ONCE — or `None` when it owes none yet.
    ///
    /// The work is materialised exactly like the status transition that started it: it fell
    /// due when the clock moved the job into the status that says it has begun, whether or
    /// not anybody called, so the first operation that finds the job begun takes the run
    /// ([`model_invocation_run::run_batch_inference`]) and no later call takes it again —
    /// which is what keeps a read reporting the same counts and leaves what a client
    /// fetched out of the output location as it fetched it.
    ///
    /// A job nothing is known about the input of owes nothing: there are no records to run
    /// over ([`Self::total_record_count`]).
    pub fn due_run(&mut self) -> Option<model_invocation_run::DueRun> {
        if !self.run_owed || !self.has_begun() || self.total_record_count.is_none() {
            return None;
        }
        self.run_owed = false;
        Some(model_invocation_run::DueRun {
            arn: self.arn.clone(),
            region: self.region.clone(),
            model_id: self.model_id.clone(),
            invocation_type: self.model_invocation_type,
            input_uri: model_invocation_run::input_s3_uri(&self.input_data_config).to_string(),
            output_uri: model_invocation_run::output_s3_uri(&self.output_data_config).to_string(),
        })
    }

    /// Write back what the run established, and give the run back when it established
    /// nothing.
    ///
    /// The counts are the run's own tally over the records it read, so what the two reads
    /// report accounts for the results stored under the output prefix. A run that could not
    /// be performed at all leaves the job still owing it, so a later call takes it again
    /// rather than reporting a job that did nothing as finished.
    ///
    /// The terminal status this job ends in is decided here; a job the clock has ALREADY
    /// carried into its terminal status is moved to the one the run decided, so no job
    /// reports `Completed` with records that failed.
    pub fn record_run(&mut self, outcome: model_invocation_run::RunOutcome) {
        match outcome {
            model_invocation_run::RunOutcome::Ran {
                success,
                error,
                message,
            } => {
                self.success_record_count = success;
                self.error_record_count = error;
                self.processed_record_count = success.saturating_add(error);
                self.terminal_status = if error == 0 {
                    MODEL_INVOCATION_JOB_STATUS_COMPLETED
                } else if success == 0 {
                    MODEL_INVOCATION_JOB_STATUS_FAILED
                } else {
                    MODEL_INVOCATION_JOB_STATUS_PARTIALLY_COMPLETED
                };
                self.message = message;
            }
            model_invocation_run::RunOutcome::Failed(message) => {
                self.terminal_status = MODEL_INVOCATION_JOB_STATUS_FAILED;
                self.message = Some(message);
            }
            model_invocation_run::RunOutcome::Deferred => {
                self.run_owed = true;
                return;
            }
        }
        if self.has_begun() && self.completed_due_at.is_none() {
            self.status = self.terminal_status;
        }
    }

    /// Materialise EVERY status transition the clock has already made due at `now`.
    ///
    /// The progression runs on the SUBMISSION's own timeline: the submission fixed the
    /// instant of each stage ([`ModelInvocationJobClock::armed_at`]) and nothing re-counts
    /// them, so what a job's status is at `now` is decided by the time elapsed since it was
    /// submitted and by nothing else — not by how many times, or how often, anybody read it.
    /// A job read once, long enough after its submission, is therefore answered with the
    /// status that elapsed time owes it, which for a job whose run has fallen due is a
    /// terminal one; a job polled tightly observes the intermediate stages on the way there,
    /// because each stage still dwells its own span. Intermediate statuses are observable,
    /// never waypoints a caller has to poll through.
    ///
    /// The stages are SEQUENTIAL and their instants strictly increasing, so the walk stops at
    /// the first stage still ahead of `now` and never materialises a later stage before an
    /// earlier one. The stop a caller asked for is owed the same way, from the instant
    /// [`Self::stop`] recorded.
    ///
    /// No move is materialised before its span has elapsed, which is why the instant it was
    /// owed at — not `now` — is the one the job reports as its last modification and its
    /// end, and why settling twice over reports the same instants.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.validating_due_at {
            if now < due {
                return;
            }
            self.status = MODEL_INVOCATION_JOB_STATUS_VALIDATING;
            self.validating_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.scheduled_due_at {
            if now < due {
                return;
            }
            self.status = MODEL_INVOCATION_JOB_STATUS_SCHEDULED;
            self.scheduled_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.in_progress_due_at {
            if now < due {
                return;
            }
            self.status = MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS;
            self.in_progress_due_at = None;
            self.last_modified_at = due;
        }
        if let Some(due) = self.completed_due_at {
            if now < due {
                return;
            }
            // The terminal status is the one the WORK decided ([`Self::terminal_status`]): a
            // job every record of which was processed completed, one some of whose records
            // could not be processed completed in part, and one that could not be run at all
            // failed with the message it failed on. The run itself is taken by the same call
            // that materialises this ([`Self::due_run`]), so the counts and the objects under
            // the output prefix are there for the read that first reports a terminal status.
            self.status = self.terminal_status;
            self.completed_due_at = None;
            self.ended_at = Some(due);
            self.last_modified_at = due;
            return;
        }
        if let Some(due) = self.stopped_due_at.filter(|due| now >= *due) {
            self.status = MODEL_INVOCATION_JOB_STATUS_STOPPED;
            self.stopped_due_at = None;
            self.ended_at = Some(due);
            self.last_modified_at = due;
        }
    }
}
