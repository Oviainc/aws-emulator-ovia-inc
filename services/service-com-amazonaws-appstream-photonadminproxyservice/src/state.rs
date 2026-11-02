// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appstream-photonadminproxyservice service.
//!
//! The stack/fleet lifecycle lives here: `CreateStack`/`CreateFleet` write a record,
//! `UpdateStack` applies the members it is given to the stored record and clears the
//! fields its `AttributesToDelete` names, `DescribeStacks`/`DescribeFleets` render the
//! stored record back, `AssociateFleet` records the fleet/stack edge,
//! `ListAssociatedFleets` reports the edges a stack has, and `DeleteStack`/`DeleteFleet`
//! refuse while an edge still names the resource.
//!
//! Usage reporting lives here too: it is ONE account-and-Region-wide setting, so
//! `CreateUsageReportSubscription` writes the single [`UsageReportSubscriptionRecord`]
//! this struct holds and `DescribeUsageReportSubscriptions` renders that same record
//! back.
//!
//! The user pool lives here as well: `CreateUser` writes a [`UserRecord`] under the
//! `(AuthenticationType, UserName)` identity it allocates, and `DescribeUsers` renders
//! the stored records of one authentication type back, a page at a time.
//!
//! Which users may reach which stacks lives here too: `BatchAssociateUserStack` writes
//! one edge per association it applies, `BatchDisassociateUserStack` removes the edges it
//! is given, and `DescribeUserStackAssociations` reports the edges matching the stack or
//! the user identity it is asked about, a page at a time.
//!
//! Records hold the MODELLED sub-shapes (`StorageConnector`, `UserSetting`,
//! `VpcConfig`, …) rather than copies of them, so what a Describe returns is literally
//! what the Create persisted.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AccessEndpoint, AgentAccessConfig, ApplicationSettingsResponse, ComputeCapacityStatus,
    ContentRedirection, DomainJoinInfo, Fleet, FleetError, LastReportGenerationExecutionError,
    S3Location, Stack, StackError, StorageConnector, StreamingExperienceSettings,
    UsageReportSubscription, User, UserSetting, UserStackAssociation, UserStackAssociationError,
    VolumeConfig, VpcConfig,
};

/// The ARN namespace this service stamps on every resource ARN
/// (`OPERATIONS.md`: "ARN namespace: `appstream`").
pub const ARN_NAMESPACE: &str = "appstream";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own shared config declares.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `FleetType` a `CreateFleet` request that omits it gets. The model's doc for
/// `CreateFleet` states the service default is ON_DEMAND.
pub const DEFAULT_FLEET_TYPE: &str = "ON_DEMAND";

/// The `FleetState` a freshly created fleet carries: a fleet is created stopped and
/// starts streaming only once `StartFleet` runs, so this is a terminal status the clock
/// owes no transition out of.
pub const FLEET_STATE_CREATED: &str = "STOPPED";

/// How many records one `DescribeStacks`/`DescribeFleets` page carries.
///
/// The model declares `NextToken` on both operations but publishes no page limit, so
/// this is the emulator's own page size; what matters on the wire is that a caller who
/// follows `NextToken` sees every record exactly once, in a stable order.
pub const DESCRIBE_PAGE_SIZE: usize = 50;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores (stacks, fleets, fleet/stack associations, user pool users).
///
/// This is NOT AWS's service quota — AppStream's published stack and fleet limits are
/// per-account and adjustable, and nothing here claims to reproduce them. It is the
/// emulator's own memory ceiling: every successful `CreateStack`, `CreateFleet`,
/// `AssociateFleet`, `CreateUser` and `BatchAssociateUserStack` inserts a record that
/// lives until a caller deletes it, so without a ceiling a caller could grow the process
/// until the host runs out of memory. Each of those handlers checks the store it writes
/// against this ceiling before inserting, and a request that would cross it is refused
/// with `LimitExceededException`, the modelled error AWS itself uses for "the requested
/// limit exceeds the permitted limit for an account", so the refusal is one a real client
/// already handles. A batch operation, whose modelled result reports per-item failures
/// instead of refusing the whole call, reports it as a per-item error
/// ([`ASSOCIATION_ERROR_INTERNAL_ERROR`]) carrying that same sentence.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with; `store`
/// names the records it holds, e.g. `"stacks"`.
pub fn limit_exceeded_message(store: &str) -> String {
    format!(
        "The requested limit exceeds the permitted limit for an account: \
         this emulator holds at most {MAX_RECORDS_PER_STORE} {store}"
    )
}

/// The word AWS's refusal sentences use for a stack (`"stack"`), the `<kind>` slot of
/// [`resource_not_found_message`] and [`resource_already_exists_message`].
pub const STACK_KIND: &str = "stack";

/// The word AWS's refusal sentences use for a fleet.
pub const FLEET_KIND: &str = "fleet";

/// AWS's `ResourceAlreadyExistsException` sentence for a user pool identity that is
/// already taken.
///
/// Unlike the named control-plane resources above, this sentence has no variable part:
/// it states that the username is taken without echoing it back, so the refusal carries
/// nothing from the request.
pub const USER_ALREADY_EXISTS_MESSAGE: &str = "A user with that username already exists.";

/// The only `AuthenticationType` the user pool operations accept: the model's docs for
/// `CreateUser.AuthenticationType` and `DescribeUsers.AuthenticationType` both state
/// "You must specify USERPOOL", because the users these operations manage are the
/// account's user pool users rather than directory or API identities.
pub const USER_POOL_AUTHENTICATION_TYPE: &str = "USERPOOL";

/// The `Status` a freshly created user pool user carries. `User.Status`'s documented
/// values include "UNCONFIRMED - The user is created but not confirmed": a user becomes
/// CONFIRMED by setting their own password from the welcome email, which no clock makes
/// due, so this is where a created user stays until that happens.
pub const USER_STATUS_UNCONFIRMED: &str = "UNCONFIRMED";

/// AWS's sentence for a user pool identity that is not in the pool.
///
/// The mirror image of [`USER_ALREADY_EXISTS_MESSAGE`], and variable-part-free for the
/// same reason: the sentence states that no user has that username without echoing the
/// username back, so the refusal carries nothing from the request.
pub const USER_NOT_FOUND_MESSAGE: &str = "A user with that username does not exist.";

/// The `UserStackAssociationError.ErrorCode` a batch reports for an association naming a
/// stack this account does not hold, one of `USER_STACK_ASSOCIATION_ERROR_CODE_VALUES`
/// (`ops/types/shared_types.rs`).
pub const ASSOCIATION_ERROR_STACK_NOT_FOUND: &str = "STACK_NOT_FOUND";

/// The `ErrorCode` a batch reports for an association naming a user the pool does not
/// hold.
pub const ASSOCIATION_ERROR_USER_NAME_NOT_FOUND: &str = "USER_NAME_NOT_FOUND";

/// The `ErrorCode` a batch reports for an association it could not apply for a reason on
/// the service's side rather than in the request — here, the association store standing
/// at [`MAX_RECORDS_PER_STORE`].
pub const ASSOCIATION_ERROR_INTERNAL_ERROR: &str = "INTERNAL_ERROR";

/// How many user/stack associations one `DescribeUserStackAssociations` page carries.
///
/// The model caps `UserStackAssociationList` at 25 members and that list is what a page
/// of the read is rendered into, so a page holds at most 25 associations however large a
/// `MaxResults` a request asks for.
pub const USER_STACK_ASSOCIATION_PAGE_SIZE: usize = 25;

/// The sentence a request refused for naming an authentication type the user pool
/// operations do not manage carries; the only variable part is the value the request
/// asked for.
pub fn authentication_type_not_user_pool_message(authentication_type: &str) -> String {
    format!(
        "AuthenticationType must be {USER_POOL_AUTHENTICATION_TYPE} for user pool \
         operations, was {authentication_type}"
    )
}

/// AWS's `ResourceNotFoundException` sentence for a named control-plane resource:
/// `The <kind> <name> for account <account id> does not exist.`
///
/// `kind` is the resource word ([`STACK_KIND`], [`FLEET_KIND`]) and `name` the name the
/// request asked for; the account id is this emulator's own, so the sentence names the
/// account that was searched rather than a value from any one run.
pub fn resource_not_found_message(kind: &str, name: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    format!("The {kind} {name} for account {account} does not exist.")
}

/// AWS's `ResourceAlreadyExistsException` sentence for a name already taken:
/// `The <kind> <name> for account <account id>  already exists. Use a unique name within
/// an AWS region in your AWS account.`
///
/// The two spaces after the account id are AWS's: the sentence is built from a template
/// whose slot between the account and `already exists` is empty, and the doubled space
/// survives into the message clients see.
pub fn resource_already_exists_message(kind: &str, name: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    format!(
        "The {kind} {name} for account {account}  already exists. \
         Use a unique name within an AWS region in your AWS account."
    )
}

/// AWS's `ResourceInUseException` sentence for a fleet a stack still uses: it states the
/// association and the remediation, and names no stack — AWS does not enumerate the
/// stacks the fleet is associated with. The only variable part is the fleet name the
/// request asked for.
pub fn fleet_in_use_by_stack_message(fleet_name: &str) -> String {
    format!(
        "The fleet {fleet_name} is associated with a stack. To delete the fleet, \
         you must dissociate it from the stack."
    )
}

/// AWS's `ResourceInUseException` sentence for a stack that still has fleets associated
/// with it: it states what blocks the delete and what makes a stack deletable, and names
/// no fleet. The only variable part is the stack name the request asked for.
pub fn stack_in_use_by_fleets_message(stack_name: &str) -> String {
    format!(
        "Cannot delete stack {stack_name}. This stack has fleets associated with it. \
         Only stacks with no fleets associated with it can be deleted."
    )
}

/// The `Schedule` a usage report subscription carries. The model's
/// `UsageReportSchedule` enum has the single value `DAILY`
/// (`USAGE_REPORT_SCHEDULE_VALUES` in `ops/types/shared_types.rs`), matching the
/// operation's documented "Usage reports are generated daily".
pub const USAGE_REPORT_SCHEDULE: &str = "DAILY";

/// The region to stamp into the ARNs a request mints: the SigV4 credential scope's
/// region, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:appstream:<region>:<account>:stack/<name>`.
pub fn stack_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("stack/{name}"))
}

/// `arn:aws:appstream:<region>:<account>:fleet/<name>`.
pub fn fleet_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("fleet/{name}"))
}

/// `arn:aws:appstream:<region>:<account>:user/<user pool type>/<user name>`.
///
/// AppStream's `user` resource ARN names the user pool an identity belongs to before
/// the user's own id, and that pool segment is the authentication type in lower case.
pub fn user_arn(region: &str, authentication_type: &str, user_name: &str) -> String {
    let pool = authentication_type.to_lowercase();
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("user/{pool}/{user_name}"))
}

/// The S3 bucket a usage report subscription stores its generated reports in.
///
/// AppStream names the bucket it creates for log and usage-report output
/// `appstream-logs-<region>-<account>-<identifier>`, where the trailing identifier is
/// what makes the name globally unique. The model documents that bucket as "unique to
/// your account and Region", so the identifier here is a digest of exactly those two:
/// enabling usage reporting again for the same account and Region reports the SAME
/// bucket rather than a name that changes from run to run.
pub fn usage_report_bucket_name(region: &str) -> String {
    let account = ::emulator_server::emulator_account_id();
    let digest =
        <::sha2::Sha256 as ::sha2::Digest>::digest(format!("appstream-logs:{region}:{account}"));
    let identifier: String = hex::encode(digest).chars().take(8).collect();
    format!("appstream-logs-{region}-{account}-{identifier}")
}

/// The instant a record is created, as the wire type the modelled `CreatedTime`
/// members carry.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A persisted AppStream stack.
#[derive(Debug, Clone)]
pub struct StackRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub storage_connectors: Vec<StorageConnector>,
    pub redirect_url: Option<String>,
    pub feedback_url: Option<String>,
    pub user_settings: Vec<UserSetting>,
    pub application_settings: Option<ApplicationSettingsResponse>,
    pub access_endpoints: Vec<AccessEndpoint>,
    pub embed_host_domains: Vec<String>,
    pub streaming_experience_settings: Option<StreamingExperienceSettings>,
    pub content_redirection: Option<ContentRedirection>,
    pub agent_access_config: Option<AgentAccessConfig>,
    pub storage_connector_authentication: Option<String>,
    pub stack_errors: Vec<StackError>,
    pub tags: BTreeMap<String, String>,
}

/// Render a stored configuration collection the way the resource reports it: a
/// collection with no entries is a configuration the stack does not have, so the member
/// is ABSENT from the rendered resource rather than present-and-empty.
///
/// This is the representation side of clearing a stack attribute: once an `UpdateStack`
/// request drops a stack's access endpoints, user settings, embedded host domains or
/// storage connectors, the corresponding key is gone from the returned `Stack` and from
/// every later `DescribeStacks`, exactly as it is for a stack that was never given one.
fn reported<T: Clone>(entries: &[T]) -> Option<Vec<T>> {
    (!entries.is_empty()).then(|| entries.to_vec())
}

impl StackRecord {
    /// Render the stored record as the modelled `Stack` every reader returns.
    pub fn to_stack(&self) -> Stack {
        Stack {
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            display_name: self.display_name.clone(),
            created_time: Some(self.created_time),
            storage_connectors: reported(&self.storage_connectors),
            redirect_url: self.redirect_url.clone(),
            feedback_url: self.feedback_url.clone(),
            // A stack always reports its error list; an empty one says "no errors",
            // which is a value, not a missing configuration.
            stack_errors: Some(self.stack_errors.clone()),
            user_settings: reported(&self.user_settings),
            application_settings: self.application_settings.clone(),
            access_endpoints: reported(&self.access_endpoints),
            embed_host_domains: reported(&self.embed_host_domains),
            streaming_experience_settings: self.streaming_experience_settings.clone(),
            content_redirection: self.content_redirection.clone(),
            agent_access_config: self.agent_access_config.clone(),
            storage_connector_authentication: self.storage_connector_authentication.clone(),
        }
    }
}

/// A persisted AppStream fleet.
#[derive(Debug, Clone)]
pub struct FleetRecord {
    pub arn: String,
    pub name: String,
    pub instance_type: String,
    pub fleet_type: String,
    pub image_name: Option<String>,
    pub image_arn: Option<String>,
    /// One of `FLEET_STATE_VALUES`; `STOPPED` for a freshly created fleet.
    pub state: String,
    pub desired_instances: i32,
    pub desired_user_sessions: Option<i32>,
    pub running: i32,
    pub in_use: i32,
    pub available: i32,
    pub vpc_config: Option<VpcConfig>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub max_user_duration_in_seconds: Option<i32>,
    pub disconnect_timeout_in_seconds: Option<i32>,
    pub idle_disconnect_timeout_in_seconds: Option<i32>,
    pub max_concurrent_sessions: Option<i32>,
    pub max_sessions_per_instance: Option<i32>,
    pub stream_view: Option<String>,
    pub platform: Option<String>,
    pub enable_default_internet_access: Option<bool>,
    pub domain_join_info: Option<DomainJoinInfo>,
    pub iam_role_arn: Option<String>,
    pub usb_device_filter_strings: Vec<String>,
    pub session_script_s3_location: Option<S3Location>,
    pub root_volume_config: Option<VolumeConfig>,
    pub disable_imdsv1: Option<bool>,
    pub created_time: ::aws_smithy_types::DateTime,
    pub fleet_errors: Vec<FleetError>,
    pub tags: BTreeMap<String, String>,
}

impl FleetRecord {
    /// Render the stored record as the modelled `Fleet` every reader returns.
    pub fn to_fleet(&self) -> Fleet {
        Fleet {
            arn: self.arn.clone(),
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            image_name: self.image_name.clone(),
            image_arn: self.image_arn.clone(),
            instance_type: self.instance_type.clone(),
            fleet_type: Some(self.fleet_type.clone()),
            compute_capacity_status: ComputeCapacityStatus {
                desired: self.desired_instances,
                running: Some(self.running),
                in_use: Some(self.in_use),
                available: Some(self.available),
                desired_user_sessions: self.desired_user_sessions,
                ..ComputeCapacityStatus::default()
            },
            max_user_duration_in_seconds: self.max_user_duration_in_seconds,
            disconnect_timeout_in_seconds: self.disconnect_timeout_in_seconds,
            state: self.state.clone(),
            vpc_config: self.vpc_config.clone(),
            created_time: Some(self.created_time),
            fleet_errors: Some(self.fleet_errors.clone()),
            enable_default_internet_access: self.enable_default_internet_access,
            domain_join_info: self.domain_join_info.clone(),
            idle_disconnect_timeout_in_seconds: self.idle_disconnect_timeout_in_seconds,
            iam_role_arn: self.iam_role_arn.clone(),
            stream_view: self.stream_view.clone(),
            platform: self.platform.clone(),
            max_concurrent_sessions: self.max_concurrent_sessions,
            usb_device_filter_strings: Some(self.usb_device_filter_strings.clone()),
            session_script_s3_location: self.session_script_s3_location.clone(),
            max_sessions_per_instance: self.max_sessions_per_instance,
            root_volume_config: self.root_volume_config.clone(),
            disable_imdsv1: self.disable_imdsv1,
        }
    }
}

/// The account's usage report subscription: the daily usage reports it has enabled, and
/// where they are written.
///
/// There is no name or id in the modelled shape because usage reporting is a single
/// account-and-Region-wide setting — hence one record, not a map.
#[derive(Debug, Clone)]
pub struct UsageReportSubscriptionRecord {
    pub s3_bucket_name: String,
    /// One of `USAGE_REPORT_SCHEDULE_VALUES`; [`USAGE_REPORT_SCHEDULE`] for a
    /// subscription this emulator created.
    pub schedule: String,
    /// When the last usage report was generated. `None` until one has been: reports are
    /// generated daily, so a freshly enabled subscription has none, and this emulator
    /// never claims a report it did not produce.
    pub last_generated_report_date: Option<::aws_smithy_types::DateTime>,
    /// The errors that stopped a usage report being generated; empty while none has.
    pub subscription_errors: Vec<LastReportGenerationExecutionError>,
}

impl UsageReportSubscriptionRecord {
    /// Render the stored record as the modelled `UsageReportSubscription` a reader
    /// returns.
    pub fn to_usage_report_subscription(&self) -> UsageReportSubscription {
        UsageReportSubscription {
            s3_bucket_name: Some(self.s3_bucket_name.clone()),
            schedule: Some(self.schedule.clone()),
            last_generated_report_date: self.last_generated_report_date,
            // "The errors that were returned if usage reports couldn't be generated":
            // with no error to report the member is a configuration the subscription
            // does not have, so it is ABSENT rather than present-and-empty.
            subscription_errors: reported(&self.subscription_errors),
        }
    }
}

/// A persisted user pool user.
///
/// The identity is the `(AuthenticationType, UserName)` pair this record is stored
/// under: email addresses are case-sensitive, so two names differing only in
/// capitalisation are two users.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub arn: String,
    pub user_name: String,
    /// One of `AUTHENTICATION_TYPE_VALUES`; [`USER_POOL_AUTHENTICATION_TYPE`] for a user
    /// this emulator created.
    pub authentication_type: String,
    /// Whether the user may sign in. A created user is enabled; `DisableUser` and
    /// `EnableUser` move it.
    pub enabled: bool,
    /// The user's status in the user pool, one of `User.Status`'s documented values;
    /// [`USER_STATUS_UNCONFIRMED`] for a user this emulator created.
    pub status: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub created_time: ::aws_smithy_types::DateTime,
}

impl UserRecord {
    /// Render the stored record as the modelled `User` every reader returns.
    pub fn to_user(&self) -> User {
        User {
            arn: Some(self.arn.clone()),
            user_name: Some(self.user_name.clone()),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            created_time: Some(self.created_time),
            authentication_type: self.authentication_type.clone(),
        }
    }
}

/// The identity one user/stack association is stored under:
/// `(AuthenticationType, UserName, StackName)`.
///
/// All three members are required on the modelled association, and together they are
/// what a `BatchDisassociateUserStack` names to remove an edge, so they are the key.
/// Ordering the authentication type and the user name before the stack name puts one
/// user's associations in a contiguous range.
pub type UserStackAssociationKey = (String, String, String);

/// The key the given association is stored under.
pub fn user_stack_association_key(association: &UserStackAssociation) -> UserStackAssociationKey {
    (
        association.authentication_type.clone(),
        association.user_name.clone(),
        association.stack_name.clone(),
    )
}

/// The per-item failure a batch reports for one association it did not apply: the
/// association as the request gave it, the modelled error code, and the sentence for that
/// error class.
pub fn user_stack_association_error(
    association: UserStackAssociation,
    error_code: &str,
    error_message: impl Into<String>,
) -> UserStackAssociationError {
    UserStackAssociationError {
        user_stack_association: Some(association),
        error_code: Some(error_code.to_string()),
        error_message: Some(error_message.into()),
    }
}

/// Which associations a read reports: every member left `None` filters nothing, so a
/// read asked only about a stack reports that stack's associations whoever the user is.
#[derive(Debug, Default)]
pub struct UserStackAssociationFilter<'a> {
    pub stack_name: Option<&'a str>,
    pub user_name: Option<&'a str>,
    pub authentication_type: Option<&'a str>,
}

impl UserStackAssociationFilter<'_> {
    /// Whether the association stored under `key` is one this filter reports.
    fn matches(&self, key: &UserStackAssociationKey) -> bool {
        let (authentication_type, user_name, stack_name) = key;
        self.stack_name.is_none_or(|name| name == stack_name)
            && self.user_name.is_none_or(|name| name == user_name)
            && self
                .authentication_type
                .is_none_or(|kind| kind == authentication_type)
    }
}

/// Per-service state for com-amazonaws-appstream-photonadminproxyservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// Stacks by `Name`. `BTreeMap` so every Describe page walks a stable order.
    pub stacks: BTreeMap<String, StackRecord>,
    /// Fleets by `Name`.
    pub fleets: BTreeMap<String, FleetRecord>,
    /// `(FleetName, StackName)` edges written by `AssociateFleet`.
    pub fleet_stack_associations: BTreeSet<(String, String)>,
    /// The account's usage report subscription, once one has been created; `None` while
    /// usage reporting is not enabled.
    pub usage_report_subscription: Option<UsageReportSubscriptionRecord>,
    /// Users by `(AuthenticationType, UserName)` — the identity a user pool user is
    /// created under. `BTreeMap` so every page of a read walks a stable order, and so
    /// one authentication type's users form a contiguous range.
    pub users: BTreeMap<(String, String), UserRecord>,
    /// User/stack associations by [`UserStackAssociationKey`] — one edge per user
    /// entitled to one stack, written by `BatchAssociateUserStack` and removed by
    /// `BatchDisassociateUserStack`.
    ///
    /// The stored value is the MODELLED association rather than a copy of its members, so
    /// what `DescribeUserStackAssociations` renders is literally what the batch
    /// persisted. `BTreeMap` so every page of a read walks a stable order.
    pub user_stack_associations: BTreeMap<UserStackAssociationKey, UserStackAssociation>,
}

impl ComAmazonawsAppstreamPhotonadminproxyserviceState {
    /// Whether any association still names this fleet. A predicate, not a list: an
    /// in-use refusal states that the fleet is associated with a stack without naming
    /// the stacks, so nothing needs the names materialised.
    pub fn fleet_is_associated(&self, fleet_name: &str) -> bool {
        self.fleet_stack_associations
            .range((fleet_name.to_string(), String::new())..)
            .take_while(|(fleet, _)| fleet == fleet_name)
            .next()
            .is_some()
    }

    /// Whether any association still names this stack.
    pub fn stack_is_associated(&self, stack_name: &str) -> bool {
        self.fleet_stack_associations
            .iter()
            .any(|(_, stack)| stack == stack_name)
    }

    /// One page of this authentication type's users: the records to render, and the
    /// token that resumes after them. `None` means `next_token` is not a token this
    /// service minted.
    ///
    /// The users are read straight from the stored map in key order, so a page renders
    /// exactly the records the store holds for that authentication type — there is no
    /// second lookup that could miss one — and following the token walks every user
    /// exactly once even when users are added between calls.
    pub fn users_page(
        &self,
        authentication_type: &str,
        next_token: Option<&str>,
        page_size: usize,
    ) -> Option<UserPage> {
        let resume_at = match next_token {
            Some(token) => decode_cursor(token)?,
            None => String::new(),
        };
        let mut remaining = self
            .users
            .range((authentication_type.to_string(), resume_at)..)
            .take_while(|((stored_type, _), _)| stored_type == authentication_type)
            .map(|(_, record)| record);
        // A page holds at least one record, so following the token always advances.
        let users: Vec<User> = remaining
            .by_ref()
            .take(page_size.max(1))
            .map(UserRecord::to_user)
            .collect();
        Some(UserPage {
            next_token: remaining
                .next()
                .map(|record| encode_cursor(&record.user_name)),
            users,
        })
    }

    /// The fleets associated with the named stack, in the stable order a page of
    /// `ListAssociatedFleets` walks.
    pub fn fleets_for_stack(&self, stack_name: &str) -> BTreeSet<String> {
        self.fleet_stack_associations
            .iter()
            .filter(|(_, stack)| stack == stack_name)
            .map(|(fleet, _)| fleet.clone())
            .collect()
    }

    /// One page of the user/stack associations `filter` reports: the associations to
    /// render, and the token that resumes after them. `None` means `next_token` is not a
    /// token this service minted.
    ///
    /// The associations are read straight from the stored map in key order, so a page
    /// renders exactly the records the store holds — there is no second lookup that could
    /// miss one — and following the token walks every matching association exactly once
    /// even when associations are added between calls.
    pub fn user_stack_associations_page(
        &self,
        filter: &UserStackAssociationFilter<'_>,
        next_token: Option<&str>,
        page_size: usize,
    ) -> Option<UserStackAssociationPage> {
        let resume_at = match next_token {
            Some(token) => Some(decode_association_cursor(token)?),
            None => None,
        };
        let mut remaining = self
            .user_stack_associations
            .iter()
            .filter(|(key, _)| resume_at.as_ref().is_none_or(|from| *key >= from))
            .filter(|(key, _)| filter.matches(key));
        // A page holds at least one record, so following the token always advances.
        let associations: Vec<UserStackAssociation> = remaining
            .by_ref()
            .take(page_size.max(1))
            .map(|(_, association)| association.clone())
            .collect();
        Some(UserStackAssociationPage {
            next_token: remaining
                .next()
                .map(|(key, _)| encode_association_cursor(key)),
            associations,
        })
    }
}

/// One page of a Describe: the names to render, and the opaque token that resumes after
/// them.
///
/// `names` is sorted and deduplicated, so following `next_token` walks every record
/// exactly once even when records are added between calls.
#[derive(Debug)]
pub struct DescribePage {
    pub names: Vec<String>,
    pub next_token: Option<String>,
}

/// One page of a `DescribeUsers`: the users to render, and the opaque token that resumes
/// after them.
#[derive(Debug)]
pub struct UserPage {
    pub users: Vec<User>,
    pub next_token: Option<String>,
}

/// One page of a `DescribeUserStackAssociations`: the associations to render, and the
/// opaque token that resumes after them.
#[derive(Debug)]
pub struct UserStackAssociationPage {
    pub associations: Vec<UserStackAssociation>,
    pub next_token: Option<String>,
}

/// Encode a pagination cursor (the name of the first record of the next page).
///
/// A real AppStream `NextToken` is opaque, so the cursor is hex-encoded rather than
/// handed to the caller in the clear: a client cannot read structure into it or
/// hand-craft one. `hex` is the maintained crate for the codec.
fn encode_cursor(name: &str) -> String {
    hex::encode(name)
}

/// The inverse of [`encode_cursor`]; `None` for a token this service did not mint.
fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The byte that joins the three parts of a user/stack association cursor.
///
/// A newline cannot appear in any of them: `Username`'s pattern admits only letters,
/// marks, symbols, numbers and punctuation, and a stored stack's name only
/// `[a-zA-Z0-9_.-]`, so the joined form parses back to exactly the key it was built from.
const ASSOCIATION_CURSOR_SEPARATOR: char = '\n';

/// Encode a user/stack association cursor: the key of the first record of the next page,
/// hex-encoded for the same reason [`encode_cursor`] is.
fn encode_association_cursor(key: &UserStackAssociationKey) -> String {
    let (authentication_type, user_name, stack_name) = key;
    encode_cursor(&format!(
        "{authentication_type}{ASSOCIATION_CURSOR_SEPARATOR}{user_name}\
         {ASSOCIATION_CURSOR_SEPARATOR}{stack_name}"
    ))
}

/// The inverse of [`encode_association_cursor`]; `None` for a token this service did not
/// mint.
fn decode_association_cursor(token: &str) -> Option<UserStackAssociationKey> {
    let decoded = decode_cursor(token)?;
    let mut parts = decoded.splitn(3, ASSOCIATION_CURSOR_SEPARATOR);
    Some((
        parts.next()?.to_string(),
        parts.next()?.to_string(),
        parts.next()?.to_string(),
    ))
}

/// Cut `all` (sorted, deduplicated record names) into the page that resumes at
/// `next_token`. `None` means the token is not one this service minted.
pub fn paginate(all: &BTreeSet<String>, next_token: Option<&str>) -> Option<DescribePage> {
    let cursor = match next_token {
        Some(token) => Some(decode_cursor(token)?),
        None => None,
    };
    let mut remaining = all
        .iter()
        .filter(|name| {
            cursor
                .as_deref()
                .is_none_or(|resume_at| name.as_str() >= resume_at)
        })
        .cloned();
    let names: Vec<String> = remaining.by_ref().take(DESCRIBE_PAGE_SIZE).collect();
    Some(DescribePage {
        next_token: remaining.next().map(|name| encode_cursor(&name)),
        names,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppstreamPhotonadminproxyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppstreamPhotonadminproxyserviceState>)
}
