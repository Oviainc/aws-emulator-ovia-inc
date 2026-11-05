// SPDX-License-Identifier: CC0-1.0

//! The append-only history of the HTTPS setup calls made against one instance,
//! and what each of them is refused or reported by.

use super::*;

/// The `Operation$operationType` a `SetupInstanceHttps` call logs, the modelled
/// `OperationType` enum's own `SetupInstanceHttps` member.
pub const OPERATION_TYPE_SETUP_INSTANCE_HTTPS: &str = "SetupInstanceHttps";

/// `SetupStatus` values this slice uses, from the modelled enum: a setup is
/// `inProgress` for as long as the certificate the request asked for is still
/// being put in place, and `succeeded` once it is.
pub const SETUP_STATUS_IN_PROGRESS: &str = "inProgress";
/// See [`SETUP_STATUS_IN_PROGRESS`].
pub const SETUP_STATUS_SUCCEEDED: &str = "succeeded";

/// How many setup records one `GetSetupHistory` page carries, from that
/// operation's own documentation: "Returns detailed information for five of the
/// most recent `SetupInstanceHttps` requests that were ran on the target
/// instance."
pub const SETUP_HISTORY_PAGE_SIZE: usize = 5;

/// The length at which a `SetupInstanceHttps` domain name counts as a short one,
/// from that operation's documentation: "If you provide more than one domain name
/// in the request, at least one name must be less than or equal to 63 characters
/// in length."
pub const SETUP_DOMAIN_NAME_SHORT_LENGTH: usize = 63;

/// The most HTTPS setup records this emulator keeps, oldest dropped first.
///
/// Emulator resource guard: the history is append-only, exactly like the
/// operation log, so without a cap a long-lived emulator would grow without
/// bound.
pub const MAX_INSTANCE_HTTPS_SETUPS: usize = 2_000;

/// One `SetupInstanceHttps` request against one instance, and what became of it.
///
/// The record is what `GetSetupHistory` reports, so it holds the request as it
/// was submitted together with the instance it ran against AS IT WAS THEN: this
/// is a history, and the resource members of a history entry do not change when
/// the instance later does.
#[derive(Debug, Clone)]
pub struct StoredInstanceHttpsSetup {
    /// The operation-log entry this call wrote, which is also what identifies
    /// the record: `SetupHistory$operationId` is "A GUID that's used to identify
    /// the operation".
    pub operation_id: String,
    pub instance_name: String,
    pub domain_names: Vec<String>,
    pub certificate_provider: String,
    /// The renewal contact the request carried. Kept, never reported: the
    /// modelled `SetupRequest` has no member for it, because it "Returns
    /// information that was submitted during the `SetupInstanceHttps` request.
    /// Email information is redacted for privacy."
    pub email_address: String,
    /// The instance's ARN when the setup ran.
    pub resource_arn: String,
    /// The instance's creation time when the setup ran.
    pub resource_created_at_ms: i64,
    /// Where that instance was.
    pub resource_location: StoredLocation,
    /// A value of the modelled `SetupStatus` enum: [`SETUP_STATUS_IN_PROGRESS`]
    /// until the clock settles it into [`SETUP_STATUS_SUCCEEDED`].
    pub status: String,
    /// When the clock owes this setup its terminal `succeeded` status, while one
    /// is outstanding.
    pub settles_at_ms: Option<i64>,
}

impl StoredInstanceHttpsSetup {
    /// The `SetupHistoryPageToken` that names this record.
    ///
    /// The modelled token admits `^[A-Za-z0-9+/=]+$` at 24..40 characters, which
    /// a GUID's own hyphens do not satisfy, so the token is that GUID's 32 hex
    /// digits — still naming exactly one record, and within the modelled length.
    pub fn page_token(&self) -> String {
        self.operation_id.replace('-', "")
    }

    /// This record as the modelled `SetupHistory`.
    pub fn view(&self) -> crate::ops::types::SetupHistory {
        crate::ops::types::SetupHistory {
            operation_id: Some(self.operation_id.clone()),
            request: Some(crate::ops::types::SetupRequest {
                instance_name: Some(self.instance_name.clone()),
                domain_names: Some(self.domain_names.clone()),
                certificate_provider: Some(self.certificate_provider.clone()),
            }),
            resource: Some(crate::ops::types::SetupHistoryResource {
                name: Some(self.instance_name.clone()),
                arn: Some(self.resource_arn.clone()),
                created_at: Some(timestamp(self.resource_created_at_ms)),
                location: Some(self.resource_location.view()),
                resource_type: Some(RESOURCE_TYPE_INSTANCE.to_string()),
            }),
            // `SetupExecutionDetails` is one COMMAND run on the instance's
            // operating system, with the stdout, the stderr and the script
            // version it produced. This emulator runs no command on an instance,
            // so there is no command output to report, and inventing some would
            // claim output nothing produced. The list is reported EMPTY rather
            // than left out, so a typed caller iterating it still gets a list.
            execution_details: Some(Vec::new()),
            status: Some(self.status.clone()),
        }
    }
}

/// Accept the domain names a `SetupInstanceHttps` request carries, or refuse with
/// a message naming the member that is wrong and the constraint it broke.
///
/// Each name's own length and charset, and the list's own 1..10 length, are
/// modelled constraints the centralized walk already refused outside. What is
/// left is the rule the model cannot express, from the operation's own
/// documentation: "If you provide more than one domain name in the request, at
/// least one name must be less than or equal to 63 characters in length."
pub fn validate_setup_domain_names(domain_names: &[String]) -> Result<(), String> {
    let short = domain_names
        .iter()
        .any(|name| name.chars().count() <= SETUP_DOMAIN_NAME_SHORT_LENGTH);
    if domain_names.len() > 1 && !short {
        return Err(format!(
            "domainNames carries {} names and every one of them is longer than \
             {SETUP_DOMAIN_NAME_SHORT_LENGTH} characters: a request naming more than one \
             domain must name at least one of at most {SETUP_DOMAIN_NAME_SHORT_LENGTH} \
             characters",
            domain_names.len()
        ));
    }
    Ok(())
}
