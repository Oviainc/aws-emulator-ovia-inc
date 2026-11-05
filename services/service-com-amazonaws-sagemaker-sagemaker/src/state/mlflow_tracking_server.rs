// SPDX-License-Identifier: CC0-1.0

//! The MLflow tracking-server slice of this service's state: the
//! [`MlflowTrackingServer`] record, the defaults the service supplies for it, its
//! statuses and its clock-owed span.

use super::*;

/// The ARN resource kind an MLflow tracking server carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrackingServerArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:mlflow-tracking-server/`), and
/// named once here so the create and the describe mint and match the same ARN.
pub const TRACKING_SERVER_KIND: &str = "mlflow-tracking-server";

/// See [`MODEL_QUOTA`]. Bounds the MLflow tracking-server store the same way, and is
/// what `CreateMlflowTrackingServer` answers `ResourceLimitExceeded` past.
pub const MLFLOW_TRACKING_SERVER_QUOTA: usize = 1_000;

/// How long after `CreateMlflowTrackingServer` the clock owes the `Creating` ->
/// `Created` transition on a tracking server. Same shape as
/// [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the server straight after
/// creating it still observes `Creating`, one that waits observes `Created`. Real
/// SageMaker takes minutes; the span is compressed, never zero.
pub const TRACKING_SERVER_CREATED_AFTER: Duration = Duration::from_millis(300);

/// `TrackingServerStatus`'s members this emulator moves a tracking server through. The
/// modelled enum also carries the update, stop, start and maintenance states, which
/// `CreateMlflowTrackingServer` and `DescribeMlflowTrackingServer` do not reach. Named
/// here rather than shared with [`STATUS_CREATING`]: that one is an `EndpointStatus`
/// member, a different modelled enum that happens to spell one state the same way.
pub const TRACKING_SERVER_CREATING: &str = "Creating";
/// See [`TRACKING_SERVER_CREATING`].
pub const TRACKING_SERVER_CREATED: &str = "Created";

/// `IsTrackingServerActive`'s two members: a tracking server is reported `Active` once
/// it has finished being created, and `Inactive` while it is still coming up.
pub const TRACKING_SERVER_ACTIVE: &str = "Active";
/// See [`TRACKING_SERVER_ACTIVE`].
pub const TRACKING_SERVER_INACTIVE: &str = "Inactive";

/// The size a tracking server gets when `CreateMlflowTrackingServer` names none:
/// `TrackingServerSize`'s model doc says "The default MLflow Tracking Server
/// configuration size is \"Small\"".
pub const DEFAULT_TRACKING_SERVER_SIZE: &str = "Small";

/// An MLflow tracking server created by `CreateMlflowTrackingServer`, as
/// `DescribeMlflowTrackingServer` replays it.
///
/// `created_due_at` is the instant the clock owes the `Creating` -> `Created`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
///
/// `mlflow_version` and `weekly_maintenance_window_start` stay `None` when the create
/// named neither: the model documents no default for either, so the server reports no
/// value rather than one the request never asked for. `tags` is stored so a later
/// `ListTags` reports what the create carried; `DescribeMlflowTrackingServer` models no
/// tag member, so `ListTags` is the one reader that surfaces it.
#[derive(Debug, Clone)]
pub struct MlflowTrackingServer {
    pub name: String,
    pub arn: String,
    pub artifact_store_uri: String,
    pub size: String,
    pub mlflow_version: Option<String>,
    pub role_arn: String,
    pub automatic_model_registration: bool,
    pub weekly_maintenance_window_start: Option<String>,
    pub s3_bucket_owner_account_id: String,
    pub s3_bucket_owner_verification: bool,
    pub status: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_due_at: SystemTime,
}

impl MlflowTrackingServer {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does for an endpoint: it was due at
    /// `created_due_at` whether or not anybody described the server, so the observer
    /// makes no difference to the result, and `LastModifiedTime` moves to the instant
    /// the transition was due rather than to "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == TRACKING_SERVER_CREATING && now >= self.created_due_at {
            self.status = TRACKING_SERVER_CREATED.to_string();
            self.last_modified_time = self.created_due_at;
            return true;
        }
        false
    }

    /// `IsActive` for the status the server is in, as one of
    /// `IsTrackingServerActive`'s two modelled members: a server is active once it is
    /// `Created`, and not before.
    pub fn is_active(&self) -> &'static str {
        if self.status == TRACKING_SERVER_CREATED {
            TRACKING_SERVER_ACTIVE
        } else {
            TRACKING_SERVER_INACTIVE
        }
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another MLflow tracking server fits, i.e. the store is below
    /// [`MLFLOW_TRACKING_SERVER_QUOTA`]. A second inherent `impl` block, beside this
    /// slice's own types rather than in the one above.
    pub fn mlflow_tracking_servers_below_quota(&self) -> bool {
        self.mlflow_tracking_servers.len() < MLFLOW_TRACKING_SERVER_QUOTA
    }
}
