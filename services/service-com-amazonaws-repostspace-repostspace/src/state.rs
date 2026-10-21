// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-repostspace-repostspace service.
//!
//! The service's primary resource is the private re:Post *space*; a space owns its
//! channels. Everything a handler answers with is derived from the records kept here,
//! so a write made by one request is what the next request reads.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{SupportedEmailDomainsParameters, SupportedEmailDomainsStatus};

/// The `ProvisioningStatus` a space carries while it is still being provisioned.
pub const SPACE_STATUS_CREATING: &str = "CREATING";
/// The `ProvisioningStatus` a provisioned space carries.
pub const SPACE_STATUS_CREATED: &str = "CREATED";

/// The `ChannelStatus` a channel carries while it is still being created.
pub const CHANNEL_STATUS_CREATING: &str = "CREATING";
/// The `ChannelStatus` a created channel carries.
pub const CHANNEL_STATUS_CREATED: &str = "CREATED";

/// A newly created space is `UNCONFIGURED` until its administrators are registered.
pub const CONFIGURATION_STATUS_UNCONFIGURED: &str = "UNCONFIGURED";

/// A requested custom subdomain is `PENDING` until AWS approves it — the model's own
/// wording: "All custom subdomains must be approved by AWS before use."
pub const VANITY_DOMAIN_STATUS_PENDING: &str = "PENDING";

/// How long a freshly created space (or channel) stays in its transient `CREATING`
/// status before the clock owes it the terminal `CREATED`.
///
/// CHOSEN, not documented: AWS publishes no provisioning duration for a private
/// re:Post. The emulator contract is a non-zero span, so a caller that reads straight
/// after the create still observes the transient status and only a later read sees the
/// resource settled.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(400);

/// The domain every private re:Post subdomain — generated or custom — is issued under.
pub const SPACE_DOMAIN_SUFFIX: &str = "private.repost.aws";

/// The `resourceType` this service reports for a space in an error body.
pub const SPACE_RESOURCE_TYPE: &str = "SPACE";
/// The `resourceType` this service reports for a channel in an error body.
pub const CHANNEL_RESOURCE_TYPE: &str = "CHANNEL";

/// The `serviceCode` this service reports in a quota error.
pub const SERVICE_CODE: &str = "repostspace";
/// This emulator's name for the per-account space quota. AWS's own quota code is not in
/// the model, and the member is `@required`, so the limit is named after itself.
pub const SPACES_QUOTA_CODE: &str = "SpacesPerAccount";
/// This emulator's name for the per-space channel quota. See [`SPACES_QUOTA_CODE`].
pub const CHANNELS_QUOTA_CODE: &str = "ChannelsPerSpace";

/// The most spaces one account may hold at a time; a create beyond it is refused with
/// the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN: AWS quotas the number of private re:Posts per account but does not publish
/// the figure. The bound is what keeps the store finite — without it a caller could grow
/// this emulator's memory without limit, since each record is only released by
/// `DeleteSpace`.
pub const MAX_SPACES_PER_ACCOUNT: usize = 100;
/// The most channels one space may hold at a time, with the same contract as
/// [`MAX_SPACES_PER_ACCOUNT`].
pub const MAX_CHANNELS_PER_SPACE: usize = 100;

/// How many ids a mint tries before giving up, so no request can spin here. A candidate
/// carries 80 random bits and the store holds at most [`MAX_SPACES_PER_ACCOUNT`]
/// records, so the first candidate is free in every realistic run.
const MINT_ATTEMPTS: usize = 8;

/// The storage a `BASIC` space includes, in bytes.
///
/// CHOSEN: AWS publishes no byte figure per tier. What is modelled (and what the tests
/// pin) is that `storageLimit` is a function of the tier alone, so it follows a tier
/// change and is identical for two spaces of the same tier.
pub const BASIC_STORAGE_LIMIT: i64 = 10 * 1024 * 1024 * 1024;
/// The storage a `STANDARD` space includes, in bytes. See [`BASIC_STORAGE_LIMIT`].
pub const STANDARD_STORAGE_LIMIT: i64 = 100 * 1024 * 1024 * 1024;

/// The storage limit the given `TierLevel` includes, or `None` when the value is not a
/// tier this service models.
pub fn storage_limit_for_tier(tier: &str) -> Option<i64> {
    match tier {
        "BASIC" => Some(BASIC_STORAGE_LIMIT),
        "STANDARD" => Some(STANDARD_STORAGE_LIMIT),
        _ => None,
    }
}

/// The region the caller signed this request for, defaulting to `us-east-1` for an
/// unsigned request — the same default the emulator's own clients use.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// This service's ARN for a space: `arn:aws:repostspace:<region>:<account>:space/<id>`.
pub fn space_arn(region: &str, space_id: &str) -> String {
    ::engine::emulator_arn("repostspace", region, &format!("space/{space_id}"))
}

/// A fresh opaque lowercase-hex id of `len` characters (`len` <= 32).
fn hex_id(len: usize) -> String {
    let mut id = ::uuid::Uuid::new_v4().simple().to_string();
    id.truncate(len);
    id
}

/// The AWS-generated subdomain a space is issued alongside its custom one.
pub fn random_subdomain() -> String {
    hex_id(12)
}

/// The Identity Center application client id a space is created with.
pub fn new_client_id() -> String {
    hex_id(26)
}

/// Carry the requested email-domain parameters into the status shape a read returns.
/// `enabled` passes through (`FeatureEnableParameter` is a subset of
/// `FeatureEnableStatus`), as does the allowed-domain list.
pub fn email_domains_status(
    parameters: &SupportedEmailDomainsParameters,
) -> SupportedEmailDomainsStatus {
    SupportedEmailDomainsStatus {
        enabled: parameters.enabled.clone(),
        allowed_domains: parameters.allowed_domains.clone(),
    }
}

/// One channel of a private re:Post.
#[derive(Debug, Clone)]
pub struct Channel {
    pub channel_id: String,
    pub space_id: String,
    pub channel_name: String,
    pub channel_description: Option<String>,
    pub create_date_time: DateTime,
    pub delete_date_time: Option<DateTime>,
    pub channel_status: String,
    pub channel_roles: HashMap<String, Vec<String>>,
    /// The instant the `CREATING` -> `CREATED` transition falls due.
    pub provisioning_due_at: Instant,
}

impl Channel {
    /// Materialise the status change the clock already owes this channel.
    fn settle(&mut self, now: Instant) {
        if self.channel_status == CHANNEL_STATUS_CREATING && now >= self.provisioning_due_at {
            self.channel_status = CHANNEL_STATUS_CREATED.to_string();
        }
    }
}

/// One private re:Post (a "space"), as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Space {
    pub space_id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub subdomain: String,
    pub vanity_domain: String,
    pub vanity_domain_status: String,
    pub random_domain: String,
    pub status: String,
    pub configuration_status: String,
    pub client_id: String,
    pub identity_store_id: Option<String>,
    pub application_arn: Option<String>,
    pub customer_role_arn: Option<String>,
    pub tier: String,
    pub storage_limit: i64,
    pub user_kms_key: Option<String>,
    pub create_date_time: DateTime,
    pub delete_date_time: Option<DateTime>,
    pub user_count: i32,
    pub content_size: i64,
    pub supported_email_domains: Option<SupportedEmailDomainsStatus>,
    pub user_admins: Vec<String>,
    pub group_admins: Vec<String>,
    pub roles: HashMap<String, Vec<String>>,
    pub tags: HashMap<String, String>,
    pub channels: HashMap<String, Channel>,
    /// The instant the `CREATING` -> `CREATED` transition falls due.
    pub provisioning_due_at: Instant,
}

impl Space {
    /// Materialise the status changes the clock already owes this space and its
    /// channels. A read calls this because the transition was due at that instant
    /// whether or not anybody asked.
    fn settle(&mut self, now: Instant) {
        if self.status == SPACE_STATUS_CREATING && now >= self.provisioning_due_at {
            self.status = SPACE_STATUS_CREATED.to_string();
        }
        for channel in self.channels.values_mut() {
            channel.settle(now);
        }
    }

    /// A channel id no channel of this space holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates were all taken. Exactly 24 characters, as `ChannelId` (len 24..24)
    /// requires.
    pub fn mint_channel_id(&self) -> Option<String> {
        (0..MINT_ATTEMPTS)
            .map(|_| hex_id(24))
            .find(|candidate| !self.channels.contains_key(candidate))
    }

    /// The channel with this id, with any clock-owed status change materialised.
    pub fn channel(&mut self, channel_id: &str) -> Option<&Channel> {
        let channel = self.channels.get_mut(channel_id)?;
        channel.settle(Instant::now());
        Some(channel)
    }
}

/// Per-service state for com-amazonaws-repostspace-repostspace. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRepostspaceRepostspaceState {
    /// Every live space, keyed by its `spaceId` — the service's primary index.
    pub spaces: HashMap<String, Space>,
    /// `arn` -> `spaceId`, so an ARN-addressed operation can resolve a space.
    pub arn_index: HashMap<String, String>,
}

impl ComAmazonawsRepostspaceRepostspaceState {
    /// A space id no stored space holds, or `None` when [`MINT_ATTEMPTS`] candidates
    /// were all taken.
    pub fn mint_space_id(&self) -> Option<String> {
        (0..MINT_ATTEMPTS)
            .map(|_| hex_id(20))
            .find(|candidate| !self.spaces.contains_key(candidate))
    }

    /// Store a space and index it by ARN.
    pub fn insert_space(&mut self, space: Space) {
        self.arn_index
            .insert(space.arn.clone(), space.space_id.clone());
        self.spaces.insert(space.space_id.clone(), space);
    }

    /// The space with this id, with any clock-owed status change materialised.
    pub fn space(&mut self, space_id: &str) -> Option<&Space> {
        self.space_mut(space_id).map(|space| &*space)
    }

    /// The space with this id for mutation, with any clock-owed status change
    /// materialised first.
    pub fn space_mut(&mut self, space_id: &str) -> Option<&mut Space> {
        let space = self.spaces.get_mut(space_id)?;
        space.settle(Instant::now());
        Some(space)
    }

    /// Forget a space (and its ARN index entry), as `DeleteSpace` does.
    pub fn remove_space(&mut self, space_id: &str) -> Option<Space> {
        let space = self.spaces.remove(space_id)?;
        self.arn_index.remove(&space.arn);
        Some(space)
    }

    /// Every stored space, settled and ordered by `spaceId` — the stable order
    /// `ListSpaces` pages over.
    pub fn spaces_by_id(&mut self) -> Vec<Space> {
        let now = Instant::now();
        let mut spaces: Vec<Space> = self
            .spaces
            .values_mut()
            .map(|space| {
                space.settle(now);
                space.clone()
            })
            .collect();
        spaces.sort_by(|a, b| a.space_id.cmp(&b.space_id));
        spaces
    }

    /// The id of the space already using this subdomain, if any.
    pub fn space_id_with_subdomain(&self, subdomain: &str) -> Option<&str> {
        self.spaces
            .values()
            .find(|space| space.subdomain == subdomain)
            .map(|space| space.space_id.as_str())
    }

    /// The id of the space already using this name, if any. The model requires a
    /// space name to be unique in the account.
    pub fn space_id_with_name(&self, name: &str) -> Option<&str> {
        self.spaces
            .values()
            .find(|space| space.name == name)
            .map(|space| space.space_id.as_str())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRepostspaceRepostspaceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRepostspaceRepostspaceState>)
}
