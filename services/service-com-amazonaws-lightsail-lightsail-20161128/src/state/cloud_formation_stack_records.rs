// SPDX-License-Identifier: CC0-1.0

//! The CloudFormation stack records an export of a snapshot to Amazon EC2 is
//! turned into, and what each record reports about the stack it created.

use super::*;

/// The resource portion a CloudFormation stack record ARN carries before its id.
/// Mirrors the `Instance/<uuid>` form the modelled `Instance$arn` documentation
/// shows, with this resource's own `ResourceType` value.
pub const CLOUD_FORMATION_STACK_RECORD_ARN_RESOURCE_PREFIX: &str = "CloudFormationStackRecord/";

/// The `resourceType` a CloudFormation stack record reports, from the modelled
/// `ResourceType` enum — the value the modelled
/// `CloudFormationStackRecord$resourceType` documentation names ("The Lightsail
/// resource type (`CloudFormationStackRecord`)").
pub const RESOURCE_TYPE_CLOUD_FORMATION_STACK_RECORD: &str = "CloudFormationStackRecord";

/// The `operationType` the log entry of a `CreateCloudFormationStack` call
/// carries: the name of the call, under the one rule
/// [`OPERATION_TYPE_COPY_SNAPSHOT`] states for every call the modelled
/// `OperationType` enum declares no member for.
pub const OPERATION_TYPE_CREATE_CLOUD_FORMATION_STACK: &str = "CreateCloudFormationStack";

/// The most CloudFormation stack records this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: every `CreateCloudFormationStack`
/// call adds a record, so a call that would exceed it is refused with the
/// modelled `InvalidInputException`.
pub const MAX_CLOUD_FORMATION_STACK_RECORDS: usize = 500;

/// How many CloudFormation stack records one `GetCloudFormationStackRecords`
/// page carries.
///
/// CHOSEN, for the same reason as [`EXPORT_SNAPSHOT_RECORD_PAGE_SIZE`]: Lightsail
/// publishes no page size for this operation either, only that the response
/// carries a `nextPageToken` when more records remain.
pub const CLOUD_FORMATION_STACK_RECORD_PAGE_SIZE: usize = 10;

/// The prefix a minted CloudFormation stack record name carries, from the
/// modelled `CloudFormationStackRecord$name` documentation: "It starts with
/// `CloudFormationStackRecord` followed by a GUID." The separating hyphen keeps
/// the name inside the modelled `ResourceName` pattern `^\w[\w\-]*\w$`.
pub const CLOUD_FORMATION_STACK_RECORD_NAME_PREFIX: &str = "CloudFormationStackRecord-";

/// The destination service a CloudFormation stack record reports, as
/// `DestinationInfo$service`.
///
/// `CloudFormationStackRecord$destinationInfo` describes "the destination
/// service, which is AWS CloudFormation, and the Amazon Resource Name (ARN) of
/// the AWS CloudFormation stack", so the one value this member can carry is that
/// service, named by its own ARN namespace and endpoint prefix
/// (`cloudformation`) — the identifier this emulator names a service with
/// everywhere else.
pub const CLOUD_FORMATION_DESTINATION_SERVICE: &str = "cloudformation";

/// The prefix the name of the AWS CloudFormation stack this operation creates
/// carries.
///
/// CHOSEN: Lightsail does not document how it names that stack, only that the
/// record reports the stack's ARN — and a stack ARN carries the stack's name. So
/// the name is minted here: this prefix, which says which service created the
/// stack, followed by the record's own id, so one value identifies the stack and
/// the record that tracks it. A letter followed by letters, digits and hyphens
/// is also a legal CloudFormation stack name.
pub const CLOUD_FORMATION_STACK_NAME_PREFIX: &str = "Lightsail-";

/// The most bytes of `userData` one instance entry may carry.
///
/// `InstanceEntry$userData` is "a launch script you can create that configures a
/// server with additional user data" for the new Amazon EC2 instance, and Amazon
/// EC2 caps the user data of an instance at 16 KB in raw form (Amazon EC2 User
/// Guide, "Run commands on your Linux instance at launch": "the user data must
/// be no larger than 16 KB").
pub const MAX_USER_DATA_BYTES: usize = 16 * 1024;

/// The ARN of the CloudFormation stack record `id` in `region`.
pub fn cloud_formation_stack_record_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CLOUD_FORMATION_STACK_RECORD_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The name of the AWS CloudFormation stack the record `id` names tracks. See
/// [`CLOUD_FORMATION_STACK_NAME_PREFIX`].
pub fn cloud_formation_stack_name(id: &str) -> String {
    format!("{CLOUD_FORMATION_STACK_NAME_PREFIX}{id}")
}

/// The ARN of the AWS CloudFormation stack `stack_name`, whose own id is
/// `stack_id`, in `region`.
///
/// A CloudFormation stack ARN carries the stack's name AND the id CloudFormation
/// minted for it (`stack/<name>/<id>`), under that service's own ARN namespace —
/// the stack is CloudFormation's resource, not Lightsail's.
pub fn cloud_formation_stack_arn(region: &str, stack_name: &str, stack_id: &str) -> String {
    ::engine::emulator_arn(
        CLOUD_FORMATION_DESTINATION_SERVICE,
        region,
        &format!("stack/{stack_name}/{stack_id}"),
    )
}

/// Whether `value` is an Amazon EC2 instance type.
///
/// `InstanceEntry$instanceType` is "the instance type (`t2.micro`) to use for
/// the new Amazon EC2 instance" and targets the modelled `NonEmptyString`, whose
/// only constraint is the `\S` pattern — so the form is Amazon EC2's own: an
/// instance type name is a family and a size separated by a single dot. The
/// family is a non-empty run of lowercase ASCII letters, digits and hyphens that
/// neither begins nor ends with a hyphen, because EC2 families carry hyphens
/// (`mac2-m2pro.metal`, `c7i-flex.large`, `u7i-12tb.224xlarge` — see the sibling
/// Amazon EC2 emulator's `instance_family_of`); the size is a non-empty run of
/// lowercase ASCII letters and digits (`micro`, `2xlarge`, `metal`).
///
/// A FORM check, deliberately: Lightsail models this member as a free-form
/// `NonEmptyString`, not as Amazon EC2's `InstanceType` enum, and the catalog of
/// types a Region offers is Amazon EC2's state rather than this service's — so a
/// well-formed name that no Region offers is accepted here rather than pulling
/// the whole sibling Amazon EC2 service crate (whose `instance_type_is_modelled`
/// carries that catalog) into this one to check one string. What this refuses is
/// a value that is not an instance type NAME at all.
pub fn is_ec2_instance_type(value: &str) -> bool {
    let Some((family, size)) = value.split_once('.') else {
        return false;
    };
    let alphanumeric = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    };
    // `split_once` splits at the FIRST dot, so a value carrying a second one
    // leaves it in `size`, which `alphanumeric` refuses.
    alphanumeric(size)
        && !family.starts_with('-')
        && !family.ends_with('-')
        && family.split('-').all(alphanumeric)
}

/// One stored CloudFormation stack record: the record `CreateCloudFormationStack`
/// created, with the clock-owed transition its state is waiting on.
///
/// Every `CreateCloudFormationStack` call appends ONE record — building a second
/// Amazon EC2 instance from the same export is a second stack, and so a second
/// record — which is why these are keyed by a minted name rather than by the
/// export snapshot record's.
#[derive(Debug, Clone)]
pub struct StoredCloudFormationStackRecord {
    /// The record name this store minted — this record's store key and the
    /// `name` every read reports. See
    /// [`CLOUD_FORMATION_STACK_RECORD_NAME_PREFIX`].
    pub name: String,
    pub arn: String,
    pub created_at_ms: i64,
    /// Where the record is: the Availability Zone the instance entry asked the
    /// new Amazon EC2 instance to be created in, and that zone's region.
    pub location: StoredLocation,
    /// The current `RecordState` value: [`RECORD_STATE_STARTED`] or
    /// [`RECORD_STATE_SUCCEEDED`].
    pub state: &'static str,
    /// The state the clock owes this record, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The export snapshot record this stack was built from, captured at the
    /// instant of the call — so the record keeps describing it even if that
    /// record itself later changes.
    pub source_info: crate::ops::types::CloudFormationStackRecordSourceInfo,
    /// The ARN of the AWS CloudFormation stack this record tracks, which is what
    /// `destinationInfo` reports. See [`cloud_formation_stack_arn`].
    pub stack_arn: String,
    /// The Amazon EC2 instance type the stack was told to create, kept verbatim.
    /// No modelled read reports it — a stack record describes the stack, not its
    /// template — so it is held here as what the stack was created from.
    pub instance_type: String,
    /// The `PortInfoSourceType` the stack was told to configure the new
    /// instance's firewall from, kept verbatim. Not reported, for the same
    /// reason as [`Self::instance_type`].
    pub port_info_source: String,
    /// The launch script the stack was told to configure the new instance with,
    /// kept verbatim; absent when the entry carried none. Not reported, for the
    /// same reason as [`Self::instance_type`].
    pub user_data: Option<String>,
}

impl StoredCloudFormationStackRecord {
    /// This record as the modelled `CloudFormationStackRecord`.
    pub fn view(&self) -> crate::ops::types::CloudFormationStackRecord {
        crate::ops::types::CloudFormationStackRecord {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_CLOUD_FORMATION_STACK_RECORD.to_string()),
            state: Some(self.state.to_string()),
            // `sourceInfo` is "a list of objects describing the source of the
            // CloudFormation stack record", and one call builds its stack from
            // exactly one export snapshot record — so one entry.
            source_info: Some(vec![self.source_info.clone()]),
            // Unlike an export's destination — an Amazon EC2 resource that does
            // not exist until the export succeeds — the stack this record
            // tracks is created by the call that appended the record, and
            // `destinationInfo` is "the destination service ... and the ARN of
            // the AWS CloudFormation stack". So it is reported from the start;
            // the record's `state` is what says whether that stack has finished
            // creating the instance.
            destination_info: Some(DestinationInfo {
                id: Some(self.stack_arn.clone()),
                service: Some(CLOUD_FORMATION_DESTINATION_SERVICE.to_string()),
            }),
        }
    }

    /// The export snapshot record this stack was built from, as the record's own
    /// source description names it.
    pub fn source_export_snapshot_record_name(&self) -> Option<&str> {
        self.source_info.name.as_deref()
    }
}
