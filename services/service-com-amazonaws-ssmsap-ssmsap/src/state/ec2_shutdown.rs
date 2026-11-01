// SPDX-License-Identifier: CC0-1.0

//! Shutting down the Amazon EC2 instances a registered application runs on.
//!
//! An EC2 instance is EC2's resource, not this service's, so the only way a call here can
//! shut one down is to ask the service that owns it, over the one client every hop into
//! EC2 goes through ([`super::ec2::client`]): the AWS SDK driven over the in-process
//! callback, which routes inside this emulator, so this crate names no other service's
//! crate and nothing leaves the process. The effect is therefore observable where an
//! instance's state lives — `DescribeInstances` — rather than only in this service's own
//! records.
//!
//! Nothing is invented when EC2 cannot be asked (no EC2 mounted, or a refusal): the hop
//! is logged at `warn` and the application's own shutdown stands, because this service
//! holds no instance state of its own to move and must not report an instance state it
//! did not observe. What EC2 answered for each instance IS answered back here
//! ([`InstanceStop`]), so the stop's own operation record reports each instance's
//! milestone as the outcome it had rather than as work that completed.

/// What asking EC2 to stop ONE instance answered with.
///
/// The outcome is reported as the milestone of that instance's work on the operation the
/// stop filed ([`crate::state::OperationTarget`]), so an instance EC2 refused is reported
/// as the failure it was rather than as work that completed.
#[derive(Debug, Clone)]
pub struct InstanceStop {
    /// The `Ec2InstanceId` that was asked for.
    pub instance_id: String,
    /// How EC2 refused the stop, absent when it accepted it.
    pub refusal: Option<String>,
}

/// Ask EC2 to stop each of `instance_ids`, and answer what it said about each — in the
/// order they were asked for.
///
/// Each instance is asked for ON ITS OWN rather than as one batch: EC2 refuses a whole
/// batch when any instance in it cannot be stopped, and the instances of one application
/// are independent of each other — one that EC2 no longer holds must not keep the rest
/// of the application's instances running.
///
/// The walk is as long as the list it is handed, which is the instances ONE registration
/// named (its single `Instances` entry and the instance of each component it declared, at
/// most [`crate::state::MAX_COMPONENTS_PER_APPLICATION`] of them) — so no request can
/// make this hop more work than that.
pub async fn stop_instances(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_ids: &[String],
) -> Vec<InstanceStop> {
    if instance_ids.is_empty() {
        return Vec::new();
    }
    let client = super::ec2::client(ctx, region);
    let mut answers: Vec<InstanceStop> = Vec::with_capacity(instance_ids.len());
    for instance_id in instance_ids {
        let answer = client
            .stop_instances()
            .instance_ids(instance_id)
            .send()
            .await;
        let refusal = match answer {
            Ok(_) => None,
            Err(error) => {
                // Visible at default verbosity: an application that stopped while its
                // instance kept running is exactly what this reports, so the gap is never
                // silent.
                ::tracing::warn!(
                    %region,
                    %instance_id,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the Amazon EC2 instance of a stopped SAP application could not be shut down"
                );
                Some(refusal_detail(&error))
            }
        };
        answers.push(InstanceStop {
            instance_id: instance_id.clone(),
            refusal,
        });
    }
    answers
}

/// How EC2's refusal is worded on the milestone of the instance it refused: the error
/// code and message EC2 answered with, which is what a client that called
/// `StopInstances` itself would have been told.
///
/// An error that carries neither — the hop never reached a service that could answer it
/// — is reported as [`UNANSWERED_REFUSAL`] instead: the whole error is logged above, and
/// nothing internal to this process belongs in a response.
fn refusal_detail<E, R>(error: &::aws_sdk_ec2::error::SdkError<E, R>) -> String
where
    E: ::aws_sdk_ec2::error::ProvideErrorMetadata + ::std::error::Error + 'static,
    R: ::std::fmt::Debug,
{
    use ::aws_sdk_ec2::error::ProvideErrorMetadata;
    match (error.code(), error.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        _ => UNANSWERED_REFUSAL.to_string(),
    }
}

/// What the milestone of an instance explains its failure with when EC2 answered no
/// error of its own: the stop was asked for and was not carried out, which is all this
/// service observed.
const UNANSWERED_REFUSAL: &str = "Amazon EC2 did not accept the request to stop the instance";
