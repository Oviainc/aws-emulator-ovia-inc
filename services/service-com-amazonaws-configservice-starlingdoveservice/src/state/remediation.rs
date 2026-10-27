// SPDX-License-Identifier: CC0-1.0

//! What a started remediation execution RUNS: the action of the Systems Manager
//! automation runbook the rule's remediation configuration names, against the resource
//! the execution was started for.
//!
//! The action is performed through the OWNING service's own API, reached over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]) — the same
//! seam the configuration recorder observes resources through — so this crate names no
//! other service crate. That call is in process, so the run takes the span that call
//! takes, and the execution reports where the run has got to
//! ([`super::RemediationExecutionEntry::state_at`]) rather than a span this emulator
//! chose: a client that reads the status after the resource has been changed is told the
//! execution succeeded, not that it is still queued.
//!
//! A runbook whose action this emulator cannot perform ([`REMEDIATION_RUNBOOKS`]) changes
//! nothing rather than something made up, and its execution keeps the clock-driven arc
//! the store records for it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{
    s3_client, service_state, ComAmazonawsConfigserviceStarlingdoveserviceState,
    RemediationConfigurationEntry, S3_BUCKET_RESOURCE_TYPE,
};

/// The action one AWS-owned automation runbook performs, as this emulator performs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunbookAction {
    /// Set the versioning state of an Amazon S3 bucket (`PutBucketVersioning`).
    SetBucketVersioning,
    /// Set the default server-side encryption of an Amazon S3 bucket
    /// (`PutBucketEncryption`).
    SetBucketDefaultEncryption,
}

/// One automation runbook a remediation configuration can name, paired with the action
/// this emulator performs for it.
#[derive(Debug, Clone, Copy)]
pub struct RemediationRunbook {
    /// The document name a remediation configuration names it by
    /// (`RemediationConfiguration$TargetId`).
    pub name: &'static str,
    /// See [`RemediationRunbook`].
    pub action: RunbookAction,
}

/// The runbooks whose action this emulator PERFORMS.
///
/// Source: the "Systems Manager Automation runbook reference" entries for the AWS-owned
/// runbooks a Config remediation configuration names on an Amazon S3 bucket, whose
/// parameters are the ones named in the constants below. A runbook NOT listed here is
/// left unperformed — the execution changes no resource rather than one this emulator
/// guessed at — which is why this table is the single statement of what a remediation can
/// do here.
pub const REMEDIATION_RUNBOOKS: [RemediationRunbook; 2] = [
    RemediationRunbook {
        name: "AWS-ConfigureS3BucketVersioning",
        action: RunbookAction::SetBucketVersioning,
    },
    RemediationRunbook {
        name: "AWS-EnableS3BucketEncryption",
        action: RunbookAction::SetBucketDefaultEncryption,
    },
];

/// The parameter the S3 runbooks take the bucket's name in.
pub const BUCKET_NAME_PARAMETER: &str = "BucketName";

/// The parameter `AWS-ConfigureS3BucketVersioning` takes the versioning state in, and
/// the state it applies when the configuration names none (`Enabled` is that runbook's
/// documented default).
const VERSIONING_STATE_PARAMETER: &str = "VersioningState";
/// See [`VERSIONING_STATE_PARAMETER`].
const DEFAULT_VERSIONING_STATE: &str = "Enabled";

/// The parameter `AWS-EnableS3BucketEncryption` takes the default encryption algorithm
/// in, and the algorithm it applies when the configuration names none (`AES256` is that
/// runbook's documented default).
const SSE_ALGORITHM_PARAMETER: &str = "SSEAlgorithm";
/// See [`SSE_ALGORITHM_PARAMETER`].
const DEFAULT_SSE_ALGORITHM: &str = "AES256";

/// The `ResourceValue` that resolves to the resource the execution is about
/// (`com.amazonaws.configservice#ResourceValueType`, whose one value this is).
const RESOURCE_VALUE_RESOURCE_ID: &str = "RESOURCE_ID";

/// The runbook `target_id` names, when this emulator performs its action.
pub fn remediation_runbook_of(target_id: &str) -> Option<&'static RemediationRunbook> {
    let target_id = target_id.trim();
    REMEDIATION_RUNBOOKS
        .iter()
        .find(|runbook| runbook.name == target_id)
}

/// One run a started remediation execution owes: what to run, and against which resource.
#[derive(Debug, Clone)]
pub struct RemediationAction {
    /// The rule whose remediation this runs, which is what its execution is recorded
    /// under.
    pub config_rule_name: String,
    /// The type of the resource the execution names (`ResourceKey$resourceType`).
    pub resource_type: String,
    /// The id of the resource the execution names (`ResourceKey$resourceId`).
    pub resource_id: String,
    /// The runbook the rule's remediation configuration names
    /// (`RemediationConfiguration$TargetId`).
    pub target_id: String,
    /// The configuration's parameters, each resolved to the ONE value the runbook runs
    /// with; see [`resolved_remediation_inputs`].
    pub inputs: BTreeMap<String, String>,
}

/// The remediation parameters of `remediation`, each resolved to the one value the
/// runbook runs with for the resource `resource_id`.
///
/// A `RemediationParameterValue` "is either a dynamic (resource) value or a static
/// value": a static value contributes the value it carries, and a resource value
/// contributes the resource the execution is about, which is what `RESOURCE_ID` names. A
/// parameter this emulator cannot resolve contributes nothing, so the runbook falls back
/// to its own documented default for it.
pub fn resolved_remediation_inputs(
    remediation: &RemediationConfigurationEntry,
    resource_id: &str,
) -> BTreeMap<String, String> {
    remediation
        .parameters
        .iter()
        .filter_map(|(name, value)| {
            let resolved = match (&value.static_value, &value.resource_value) {
                (Some(static_value), _) => static_value.values.first().cloned(),
                (None, Some(resource_value))
                    if resource_value.value.trim() == RESOURCE_VALUE_RESOURCE_ID =>
                {
                    Some(resource_id.to_string())
                }
                _ => None,
            };
            Some((name.clone(), resolved?))
        })
        .collect()
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Note that the run of `action` began at `ran_from` and had performed it by
    /// `ran_to`, onto the execution the run belongs to.
    ///
    /// This is what makes the reported state follow the RUN rather than the clock; an
    /// execution the store no longer holds — one dropped past the bound it keeps, or
    /// replaced by a later start for the same resource — keeps whatever its own record
    /// says.
    pub fn note_remediation_run(
        &mut self,
        action: &RemediationAction,
        ran_from: SystemTime,
        ran_to: SystemTime,
    ) {
        let key = (
            action.config_rule_name.clone(),
            action.resource_type.clone(),
            action.resource_id.clone(),
        );
        if let Some(execution) = self.remediation_executions.get_mut(&key) {
            execution.ran_from = Some(ran_from);
            execution.ran_to = Some(ran_to);
        }
    }
}

/// Run `actions`, noting on each execution what its own run did.
///
/// Called by `StartRemediationExecution` with the store's lock RELEASED — no lock is held
/// across an await: the guard is taken again only to note each run.
pub async fn run_remediation_actions<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
    actions: &[RemediationAction],
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    for action in actions {
        let ran_from = SystemTime::now();
        if run_remediation_action(callback, region, action).await {
            state
                .lock()
                .note_remediation_run(action, ran_from, SystemTime::now());
        }
    }
}

/// Run ONE action: the step the runbook the configuration names performs, against the
/// resource the execution names; `true` when it was performed.
///
/// A runbook outside [`REMEDIATION_RUNBOOKS`], an action whose resource this emulator
/// cannot name, and a call the owning service refuses each leave the resource as it was;
/// the refusal is logged, because a remediation that cannot act is worth seeing.
async fn run_remediation_action(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    action: &RemediationAction,
) -> bool {
    let Some(runbook) = remediation_runbook_of(&action.target_id) else {
        return false;
    };
    let Some(bucket) = remediated_bucket(action) else {
        return false;
    };
    let client = s3_client(callback, region);
    let outcome = match runbook.action {
        RunbookAction::SetBucketVersioning => {
            let status = action
                .inputs
                .get(VERSIONING_STATE_PARAMETER)
                .map(String::as_str)
                .unwrap_or(DEFAULT_VERSIONING_STATE);
            client
                .put_bucket_versioning()
                .bucket(&bucket)
                .versioning_configuration(
                    ::aws_sdk_s3::types::VersioningConfiguration::builder()
                        .status(::aws_sdk_s3::types::BucketVersioningStatus::from(status))
                        .build(),
                )
                .send()
                .await
                .map(|_| ())
                .map_err(|error| {
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
                })
        }
        RunbookAction::SetBucketDefaultEncryption => {
            let algorithm = action
                .inputs
                .get(SSE_ALGORITHM_PARAMETER)
                .map(String::as_str)
                .unwrap_or(DEFAULT_SSE_ALGORITHM);
            let Ok(by_default) = ::aws_sdk_s3::types::ServerSideEncryptionByDefault::builder()
                .sse_algorithm(::aws_sdk_s3::types::ServerSideEncryption::from(algorithm))
                .build()
            else {
                return false;
            };
            let rule = ::aws_sdk_s3::types::ServerSideEncryptionRule::builder()
                .apply_server_side_encryption_by_default(by_default)
                .build();
            let Ok(configuration) =
                ::aws_sdk_s3::types::ServerSideEncryptionConfiguration::builder()
                    .rules(rule)
                    .build()
            else {
                return false;
            };
            client
                .put_bucket_encryption()
                .bucket(&bucket)
                .server_side_encryption_configuration(configuration)
                .send()
                .await
                .map(|_| ())
                .map_err(|error| {
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
                })
        }
    };
    match outcome {
        Ok(()) => true,
        Err(error) => {
            ::tracing::warn!(
                runbook = runbook.name,
                resource = %action.resource_id,
                error = %error,
                "a remediation execution could not run its target action"
            );
            false
        }
    }
}

/// The bucket an S3 runbook acts on: the one its `BucketName` parameter resolves to, or
/// the resource the execution names when that resource IS a bucket — which is what the
/// `RESOURCE_ID` resource value resolves to in the first place.
fn remediated_bucket(action: &RemediationAction) -> Option<String> {
    action
        .inputs
        .get(BUCKET_NAME_PARAMETER)
        .map(|bucket| bucket.trim().to_string())
        .filter(|bucket| !bucket.is_empty())
        .or_else(|| {
            (action.resource_type == S3_BUCKET_RESOURCE_TYPE)
                .then(|| action.resource_id.clone())
                .filter(|resource| !resource.trim().is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{
        RemediationConfiguration, RemediationParameterValue, ResourceValue, StaticValue,
    };

    /// A remediation configuration carrying `parameters`.
    fn configuration(
        parameters: Vec<(&str, RemediationParameterValue)>,
    ) -> RemediationConfigurationEntry {
        RemediationConfigurationEntry::new(
            "id".to_string(),
            "arn".to_string(),
            &RemediationConfiguration {
                config_rule_name: "rule".to_string(),
                target_type: "SSM_DOCUMENT".to_string(),
                target_id: REMEDIATION_RUNBOOKS[0].name.to_string(),
                parameters: Some(
                    parameters
                        .into_iter()
                        .map(|(name, value)| (name.to_string(), value))
                        .collect(),
                ),
                ..RemediationConfiguration::default()
            },
        )
    }

    /// A static parameter value carrying `value`.
    fn static_value(value: &str) -> RemediationParameterValue {
        RemediationParameterValue {
            static_value: Some(StaticValue {
                values: vec![value.to_string()],
            }),
            resource_value: None,
        }
    }

    /// A resource parameter value naming `value`.
    fn resource_value(value: &str) -> RemediationParameterValue {
        RemediationParameterValue {
            static_value: None,
            resource_value: Some(ResourceValue {
                value: value.to_string(),
            }),
        }
    }

    /// Each parameter resolves to the ONE value the runbook runs with: a static value to
    /// what it carries, the resource value to the resource the execution is about, and a
    /// value naming neither to nothing at all. The rows differ along which form the
    /// parameter takes.
    #[test]
    fn a_parameter_resolves_to_the_value_the_runbook_runs_with() {
        let cases: [(&str, RemediationParameterValue, Option<&str>); 4] = [
            (
                "a static value",
                static_value("Suspended"),
                Some("Suspended"),
            ),
            (
                "the resource value",
                resource_value(RESOURCE_VALUE_RESOURCE_ID),
                Some("the-bucket"),
            ),
            (
                "a resource value of another kind",
                resource_value("OTHER"),
                None,
            ),
            (
                "neither kind of value",
                RemediationParameterValue::default(),
                None,
            ),
        ];

        for (case, value, expected) in cases {
            let resolved = resolved_remediation_inputs(
                &configuration(vec![("Parameter", value)]),
                "the-bucket",
            );
            assert_eq!(
                resolved.get("Parameter").map(String::as_str),
                expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// The runbooks this emulator performs are exactly the catalogue's, resolved by the
    /// name a configuration names them by; anything else performs nothing.
    #[test]
    fn only_a_catalogued_runbook_resolves_to_an_action() {
        for runbook in REMEDIATION_RUNBOOKS {
            assert_eq!(
                remediation_runbook_of(runbook.name).map(|found| found.action),
                Some(runbook.action),
                "case {:?}: the catalogue must resolve its own entry",
                runbook.name
            );
        }
        for case in ["AWS-UpdateSSMAgent", "aws-configures3bucketversioning", ""] {
            assert!(
                remediation_runbook_of(case).is_none(),
                "case {case:?}: a runbook outside the catalogue performs no action"
            );
        }
    }

    /// The bucket an S3 runbook acts on: the `BucketName` its parameters resolve, or the
    /// bucket the execution names. The rows differ along which of the two the action
    /// carries, and along a resource that is not a bucket at all.
    #[test]
    fn an_s3_runbook_acts_on_the_bucket_its_action_names() {
        let action = |bucket_name: Option<&str>, resource_type: &str, resource_id: &str| {
            RemediationAction {
                config_rule_name: "rule".to_string(),
                resource_type: resource_type.to_string(),
                resource_id: resource_id.to_string(),
                target_id: REMEDIATION_RUNBOOKS[0].name.to_string(),
                inputs: bucket_name
                    .map(|bucket| {
                        BTreeMap::from([(BUCKET_NAME_PARAMETER.to_string(), bucket.to_string())])
                    })
                    .unwrap_or_default(),
            }
        };
        let cases: [(&str, RemediationAction, Option<&str>); 4] = [
            (
                "the parameter names the bucket",
                action(Some("named-bucket"), S3_BUCKET_RESOURCE_TYPE, "keyed-bucket"),
                Some("named-bucket"),
            ),
            (
                "no parameter, so the resource the execution names",
                action(None, S3_BUCKET_RESOURCE_TYPE, "keyed-bucket"),
                Some("keyed-bucket"),
            ),
            (
                "an empty parameter, so the resource the execution names",
                action(Some("  "), S3_BUCKET_RESOURCE_TYPE, "keyed-bucket"),
                Some("keyed-bucket"),
            ),
            (
                "a resource that is not a bucket, and no parameter",
                action(None, "AWS::EC2::Volume", "vol-0001"),
                None,
            ),
        ];

        for (case, action, expected) in cases {
            let acted_on = remediated_bucket(&action);
            assert_eq!(
                acted_on.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {acted_on:?}"
            );
        }
    }
}
