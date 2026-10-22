// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions service.
//!
//! One store holds every resource the service's operations share: identity-provider
//! registrations, the product users subscribed through them, the EC2 instances that
//! provide those subscriptions, and the tags written against a resource's ARN. The
//! list/lookup/pagination/filter helpers live here
//! too, so the operation handlers in `ops/` all resolve a resource the same way
//! instead of each growing its own copy.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Filter, IdentityProvider, IdentityProviderSummary, InstanceSummary, LicenseServerEndpoint,
    LicenseServerSettings, ProductUserSummary, ServerEndpoint, Settings,
};

/// The status a registration carries while its VPC endpoints are being provisioned.
pub const STATUS_REGISTERING: &str = "REGISTERING";
/// The status a registration carries once provisioning completed.
pub const STATUS_REGISTERED: &str = "REGISTERED";
/// The status returned by `DeregisterIdentityProvider` for the registration it removed.
pub const STATUS_DEREGISTERING: &str = "DEREGISTERING";
/// The status a product user carries while its subscription is active.
pub const STATUS_SUBSCRIBED: &str = "SUBSCRIBED";
/// The status returned by `StopProductSubscription` for the subscription it ended.
pub const STATUS_UNSUBSCRIBED: &str = "UNSUBSCRIBED";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries while its network
/// endpoint is being provisioned.
pub const PROVISIONING_STATUS_PROVISIONING: &str = "PROVISIONING";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries once provisioning
/// completed.
pub const PROVISIONING_STATUS_PROVISIONED: &str = "PROVISIONED";

/// How long after `RegisterIdentityProvider` the registration becomes `REGISTERED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the registration is returned in the
/// non-terminal `REGISTERING` status, so a caller that reads it straight away still
/// observes that transient status, and any later read materialises the transition the
/// clock already owed.
const REGISTRATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `CreateLicenseServerEndpoint` the endpoint becomes `PROVISIONED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the endpoint is created in the
/// non-terminal `PROVISIONING` status, so a caller that lists it straight away still
/// observes that transient status, and a later read materialises the transition the
/// clock already owed.
const ENDPOINT_PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// Why an operation could not be served from state.
///
/// Each handler maps this onto its OWN modelled error enum (the generated enums are
/// per-operation types), so the lookup rules themselves live here exactly once.
#[derive(Debug, Clone)]
pub enum StateError {
    /// The addressed resource is not in the store — HTTP 404 `ResourceNotFoundException`.
    NotFound(String),
    /// The resource already exists in a conflicting state — `ConflictException`.
    Conflict(String),
    /// The request is malformed in a way the model cannot express — HTTP 400
    /// `ValidationException`.
    Validation(String),
}

/// A registered identity provider, as `RegisterIdentityProvider` stored it.
#[derive(Debug, Clone)]
pub struct IdentityProviderRecord {
    /// The minted `IdentityProviderArn`; the key of `identity_providers`.
    pub arn: String,
    /// The provider union exactly as registered.
    pub identity_provider: IdentityProvider,
    /// The user-based subscription product this registration is for.
    pub product: String,
    /// The product-related configuration settings (subnets + security group).
    pub settings: Settings,
    /// `REGISTERING` until [`REGISTRATION_SPAN`] has elapsed, then `REGISTERED`.
    pub status: String,
    /// Set only when provisioning failed; never fabricated.
    pub failure_message: Option<String>,
    /// The account that owns the registration.
    pub owner_account_id: String,
    /// When the registration was created.
    pub created_at: SystemTime,
    /// When the `REGISTERING` -> `REGISTERED` transition falls due.
    pub registered_due_at: SystemTime,
}

/// An EC2 instance that provides user-based subscriptions.
///
/// An instance is not created by an API of this service: it enters the service's view
/// through the user-association plane (`AssociateUser` names the `InstanceId` a user is
/// associated with), and `ListInstances` only REPORTS what is held here. So this record
/// carries no value `ListInstances` invents — every member is what the writer persisted.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    /// The EC2 instance id; the key of `instances`.
    pub instance_id: String,
    /// The status of the instance resource, as the writer recorded it. The model
    /// (`InstanceSummary.Status`) is a free-form string, so no vocabulary is imposed here.
    pub status: String,
    /// The user-based subscription products this instance provides, in the order they
    /// were added.
    pub products: Vec<String>,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The account that owns the instance.
    pub owner_account_id: String,
    /// The registration the instance's users were resolved through, when one is known.
    pub identity_provider: Option<IdentityProvider>,
    /// When the instance's status was last checked, if it ever was. A status check is
    /// work this service performs, so a read never invents one.
    pub last_status_check_at: Option<SystemTime>,
}

/// A product user, as `StartProductSubscription` stored it.
#[derive(Debug, Clone)]
pub struct ProductUserRecord {
    /// The minted `ProductUserArn`; the key of `product_users`.
    pub arn: String,
    /// The user name from the identity provider.
    pub username: String,
    /// The user-based subscription product.
    pub product: String,
    /// The ARN of the identity provider this user was resolved through.
    pub identity_provider_arn: String,
    /// `SUBSCRIBED` while the subscription is active.
    pub status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The Active Directory domain the user information came from.
    pub domain: Option<String>,
    /// ISO-8601 instant the subscription started.
    pub subscription_start_date: Option<String>,
    /// ISO-8601 instant the subscription ended, once it has.
    pub subscription_end_date: Option<String>,
    /// Only products backed by a license server endpoint carry one, so this stays
    /// `None` for a subscription this slice creates.
    pub license_expiration_date: Option<String>,
}

/// A license server endpoint, as `CreateLicenseServerEndpoint` stored it.
#[derive(Debug, Clone)]
pub struct LicenseServerEndpointRecord {
    /// The minted `LicenseServerEndpointArn`; the key of `license_server_endpoints`.
    pub arn: String,
    /// The minted `LicenseServerEndpointId`.
    pub id: String,
    /// The registration this endpoint was created for.
    pub identity_provider_arn: String,
    /// The `ServerType` the endpoint serves (the model's only value is `RDS_SAL`).
    pub server_type: String,
    /// The license-server settings exactly as created, so nothing the caller supplied is
    /// lost between operations.
    pub settings: LicenseServerSettings,
    /// The network address of the endpoint, reported only once it is `PROVISIONED`:
    /// there is no address to reach while it is still being provisioned.
    pub endpoint: String,
    /// `PROVISIONING` until [`ENDPOINT_PROVISIONING_SPAN`] has elapsed, then `PROVISIONED`.
    pub provisioning_status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// When the endpoint was created.
    pub created_at: SystemTime,
    /// When the `PROVISIONING` -> `PROVISIONED` transition falls due.
    pub provisioned_due_at: SystemTime,
}

/// Per-service state for com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    /// Registrations by `IdentityProviderArn`.
    pub identity_providers: HashMap<String, IdentityProviderRecord>,
    /// Registration ARNs in registration order, so a list is stable and pageable.
    pub identity_provider_order: Vec<String>,
    /// `(provider key, product) -> IdentityProviderArn`, so a caller that addresses a
    /// registration by `(IdentityProvider, Product)` resolves the same record an ARN does.
    pub identity_provider_index: HashMap<(String, String), String>,
    /// Product users by `ProductUserArn`.
    pub product_users: HashMap<String, ProductUserRecord>,
    /// Product-user ARNs in subscription order, so a list is stable and pageable.
    pub product_user_order: Vec<String>,
    /// `(IdentityProviderArn, product, username) -> ProductUserArn`.
    pub product_user_index: HashMap<(String, String, String), String>,
    /// License server endpoints by `LicenseServerEndpointArn`.
    pub license_server_endpoints: HashMap<String, LicenseServerEndpointRecord>,
    /// Endpoint ARNs in creation order, so a list is stable and pageable.
    pub license_server_endpoint_order: Vec<String>,
    /// `(IdentityProviderArn, ServerType) -> LicenseServerEndpointArn`, so a repeat
    /// create resolves the endpoint already in state instead of minting a second one.
    pub license_server_endpoint_index: HashMap<(String, String), String>,
    /// Instances providing user-based subscriptions, by EC2 instance id.
    pub instances: HashMap<String, InstanceRecord>,
    /// Instance ids in the order the instances entered the store, so a list is stable
    /// and pageable.
    pub instance_order: Vec<String>,
    /// Tags by resource ARN, written by whichever operation accepted them, so a later
    /// tagging operation reads the same store.
    pub tags: HashMap<String, HashMap<String, String>>,
    /// Source of the resource ids this service mints.
    pub next_id: u64,
}

impl ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    /// Mint the resource-id portion of an ARN: `<prefix>-<12 lowercase hex digits>`.
    ///
    /// Monotonic, so ids are unique within a run and the mint order is the list order.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{:012x}", self.next_id)
    }

    /// Materialise every transition the clock already made due at `now`.
    ///
    /// Only transitions that were owed at `now` whether or not anybody called: a
    /// registration whose [`REGISTRATION_SPAN`] has elapsed becomes `REGISTERED`.
    /// Reporting operations call this so a read reflects the clock without the read
    /// itself changing anything a later caller would not have seen anyway.
    pub fn materialise_due(&mut self, now: SystemTime) {
        for record in self.identity_providers.values_mut() {
            if record.status == STATUS_REGISTERING && now >= record.registered_due_at {
                record.status = STATUS_REGISTERED.to_string();
            }
        }
        for record in self.license_server_endpoints.values_mut() {
            if record.provisioning_status == PROVISIONING_STATUS_PROVISIONING
                && now >= record.provisioned_due_at
            {
                record.provisioning_status = PROVISIONING_STATUS_PROVISIONED.to_string();
            }
        }
    }

    /// The registrations in registration order.
    pub fn identity_providers_in_order(&self) -> Vec<IdentityProviderRecord> {
        self.identity_provider_order
            .iter()
            .filter_map(|arn| self.identity_providers.get(arn).cloned())
            .collect()
    }

    /// The license server endpoints in creation order.
    pub fn license_server_endpoints_in_order(&self) -> Vec<LicenseServerEndpointRecord> {
        self.license_server_endpoint_order
            .iter()
            .filter_map(|arn| self.license_server_endpoints.get(arn).cloned())
            .collect()
    }

    /// The instances in the order they entered the store.
    pub fn instances_in_order(&self) -> Vec<InstanceRecord> {
        self.instance_order
            .iter()
            .filter_map(|id| self.instances.get(id).cloned())
            .collect()
    }

    /// Record an instance, keeping the list order the order of first appearance: a
    /// re-recorded instance updates its record in place rather than moving in the list.
    ///
    /// Every writer that brings an instance into the service's view goes through here, so
    /// `instances` and `instance_order` cannot disagree.
    pub fn upsert_instance(&mut self, record: InstanceRecord) {
        let id = record.instance_id.clone();
        if self.instances.insert(id.clone(), record).is_none() {
            self.instance_order.push(id);
        }
    }

    /// Remove an instance from the store and from the list order.
    pub fn remove_instance(&mut self, instance_id: &str) -> Option<InstanceRecord> {
        let record = self.instances.remove(instance_id)?;
        self.instance_order.retain(|id| id != instance_id);
        Some(record)
    }

    /// The product users of one registration (optionally one product), in subscription order.
    pub fn product_users_in_order(
        &self,
        identity_provider_arn: &str,
        product: Option<&str>,
    ) -> Vec<ProductUserRecord> {
        self.product_user_order
            .iter()
            .filter_map(|arn| self.product_users.get(arn))
            .filter(|record| record.identity_provider_arn == identity_provider_arn)
            .filter(|record| product.is_none_or(|p| record.product == p))
            .cloned()
            .collect()
    }

    /// Resolve a registration from the two ways every operation can address one: its
    /// ARN, or the `(IdentityProvider, Product)` pair it was registered with.
    ///
    /// `ValidationException` when neither addressing is complete, `ResourceNotFoundException`
    /// when the addressed registration is not registered.
    pub fn resolve_identity_provider(
        &self,
        arn: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
    ) -> Result<&IdentityProviderRecord, StateError> {
        if let Some(arn) = arn {
            return self.identity_providers.get(arn).ok_or_else(|| {
                StateError::NotFound(format!("identity provider {arn} is not registered"))
            });
        }
        let (Some(identity_provider), Some(product)) = (identity_provider, product) else {
            return Err(StateError::Validation(
                "address an identity provider either by IdentityProviderArn or by both \
                 IdentityProvider and Product"
                    .to_string(),
            ));
        };
        let key = provider_key(identity_provider)?;
        let arn = self
            .identity_provider_index
            .get(&(key, product.to_string()))
            .ok_or_else(|| {
                StateError::NotFound(format!(
                    "the identity provider is not registered for product {product}"
                ))
            })?;
        self.identity_providers.get(arn).ok_or_else(|| {
            StateError::NotFound(format!("identity provider {arn} is not registered"))
        })
    }

    /// Resolve a product user from the two ways `StopProductSubscription` can address
    /// one: its `ProductUserArn`, or the `(Username, IdentityProvider, Product)` triple
    /// it was subscribed with, optionally narrowed by `Domain`.
    ///
    /// `ValidationException` when neither addressing is complete, `ResourceNotFoundException`
    /// when no such subscription is in state.
    pub fn resolve_product_user(
        &self,
        product_user_arn: Option<&str>,
        username: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
        domain: Option<&str>,
    ) -> Result<&ProductUserRecord, StateError> {
        let record = if let Some(arn) = product_user_arn {
            self.product_users.get(arn).ok_or_else(|| {
                StateError::NotFound(format!("product user {arn} has no subscription"))
            })?
        } else {
            let (Some(username), Some(identity_provider), Some(product)) =
                (username, identity_provider, product)
            else {
                return Err(StateError::Validation(
                    "address a product user either by ProductUserArn or by Username, \
                     IdentityProvider and Product"
                        .to_string(),
                ));
            };
            let provider =
                self.resolve_identity_provider(None, Some(identity_provider), Some(product))?;
            let arn = self
                .product_user_index
                .get(&(
                    provider.arn.clone(),
                    product.to_string(),
                    username.to_string(),
                ))
                .ok_or_else(|| {
                    StateError::NotFound(format!(
                        "user {username} has no subscription to product {product}"
                    ))
                })?;
            self.product_users.get(arn).ok_or_else(|| {
                StateError::NotFound(format!("product user {arn} has no subscription"))
            })?
        };
        // `Domain` narrows the addressing rather than being ignored: a subscription from
        // another domain is not the one the caller named.
        if let Some(domain) = domain.filter(|d| !d.is_empty()) {
            if record.domain.as_deref() != Some(domain) {
                return Err(StateError::NotFound(format!(
                    "user {} has no subscription in domain {domain}",
                    record.username
                )));
            }
        }
        Ok(record)
    }

    /// Remove a registration, and with it the product users that were subscribed
    /// through it (they are only reachable through their provider).
    pub fn remove_identity_provider(&mut self, arn: &str) -> Option<IdentityProviderRecord> {
        let record = self.identity_providers.remove(arn)?;
        self.identity_provider_order.retain(|a| a != arn);
        self.identity_provider_index
            .retain(|_, indexed| indexed != arn);
        let orphaned: Vec<String> = self
            .product_users
            .values()
            .filter(|user| user.identity_provider_arn == arn)
            .map(|user| user.arn.clone())
            .collect();
        for user_arn in orphaned {
            self.remove_product_user(&user_arn);
        }
        self.tags.remove(arn);
        Some(record)
    }

    /// Remove one product user from the store and from every index that reaches it.
    pub fn remove_product_user(&mut self, arn: &str) -> Option<ProductUserRecord> {
        let record = self.product_users.remove(arn)?;
        self.product_user_order.retain(|a| a != arn);
        self.product_user_index.retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        Some(record)
    }
}

/// The key a registration is indexed by, derived from the provider union.
///
/// `IdentityProvider` is a union whose only member is `ActiveDirectoryIdentityProvider`,
/// which identifies a directory either by `DirectoryId` (an AWS Managed directory) or by
/// the domain name in `ActiveDirectorySettings` (a self-managed one). One of the two must
/// be present, or the provider names no directory at all and nothing can be registered
/// for it — a combination the model cannot express, so it is refused here.
pub fn provider_key(identity_provider: &IdentityProvider) -> Result<String, StateError> {
    match identity_provider {
        IdentityProvider::ActiveDirectoryIdentityProvider(ad) => {
            if let Some(directory_id) = ad.directory_id.as_deref().filter(|id| !id.is_empty()) {
                return Ok(format!("directory/{directory_id}"));
            }
            let domain = ad
                .active_directory_settings
                .as_ref()
                .and_then(|settings| settings.domain_name.as_deref())
                .filter(|name| !name.is_empty());
            match domain {
                Some(domain) => Ok(format!("domain/{domain}")),
                None => Err(StateError::Validation(
                    "IdentityProvider.ActiveDirectoryIdentityProvider must specify either \
                     DirectoryId or ActiveDirectorySettings.DomainName"
                        .to_string(),
                )),
            }
        }
    }
}

/// The `DirectoryId` of a provider union, when it names one.
///
/// `ListIdentityProviders` documents `DirectoryId` as a filterable attribute, so the
/// value a filter compares against comes from here.
pub fn directory_id(identity_provider: &IdentityProvider) -> Option<&str> {
    match identity_provider {
        IdentityProvider::ActiveDirectoryIdentityProvider(ad) => ad.directory_id.as_deref(),
    }
}

/// Format `at` the way this service's date members carry an instant: ISO-8601 UTC at
/// whole-second precision, e.g. `2025-03-15T00:00:00Z` — the format the model's
/// `LicenseExpirationDate` documentation shows.
pub fn iso8601(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The `IdentityProviderSummary` the model requires, built entirely from the stored
/// record — every member is what registration persisted.
pub fn identity_provider_summary(record: &IdentityProviderRecord) -> IdentityProviderSummary {
    IdentityProviderSummary {
        identity_provider: record.identity_provider.clone(),
        settings: record.settings.clone(),
        product: record.product.clone(),
        status: record.status.clone(),
        identity_provider_arn: Some(record.arn.clone()),
        failure_message: record.failure_message.clone(),
        owner_account_id: Some(record.owner_account_id.clone()),
    }
}

/// The `InstanceSummary` the model requires, built entirely from the stored record —
/// every member is what the writer persisted, and an absent one stays absent.
pub fn instance_summary(record: &InstanceRecord) -> InstanceSummary {
    InstanceSummary {
        instance_id: record.instance_id.clone(),
        status: record.status.clone(),
        products: record.products.clone(),
        last_status_check_date: record.last_status_check_at.map(iso8601),
        status_message: record.status_message.clone(),
        owner_account_id: Some(record.owner_account_id.clone()),
        identity_provider: record.identity_provider.clone(),
    }
}

/// The `ProductUserSummary` the model requires, built from the stored product user and
/// the provider union of the registration it was subscribed through.
pub fn product_user_summary(
    record: &ProductUserRecord,
    identity_provider: &IdentityProvider,
) -> ProductUserSummary {
    ProductUserSummary {
        username: record.username.clone(),
        product: record.product.clone(),
        identity_provider: identity_provider.clone(),
        status: record.status.clone(),
        product_user_arn: Some(record.arn.clone()),
        status_message: record.status_message.clone(),
        domain: record.domain.clone(),
        subscription_start_date: record.subscription_start_date.clone(),
        subscription_end_date: record.subscription_end_date.clone(),
        license_expiration_date: record.license_expiration_date.clone(),
    }
}

/// The region this request was signed for, or the emulator's default when the request
/// carries no usable SigV4 credential scope.
///
/// Values outside the ARN grammar's region segment (`[a-z0-9-.]{1,63}`) are ignored, so
/// a minted ARN always satisfies the model's `Arn` pattern.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        })
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// An ARN for one of this service's resources: `arn:aws:<signing name>:<region>:<account>:<kind>/<id>`.
pub fn resource_arn(signing_name: &str, region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(signing_name, region, &format!("{kind}/{id}"))
}

/// The instant a registration created at `created_at` becomes `REGISTERED`.
pub fn registration_due_at(created_at: SystemTime) -> SystemTime {
    created_at + REGISTRATION_SPAN
}

/// The instant a license server endpoint created at `created_at` becomes `PROVISIONED`.
pub fn endpoint_provisioned_due_at(created_at: SystemTime) -> SystemTime {
    created_at + ENDPOINT_PROVISIONING_SPAN
}

/// The network address of the license server endpoint `id`, in the regional form this
/// emulator mints for a service-owned endpoint: `<id>.<signing name>.<region>.amazonaws.com`.
pub fn endpoint_address(signing_name: &str, region: &str, id: &str) -> String {
    format!("{id}.{signing_name}.{region}.amazonaws.com")
}

/// The `LicenseServerEndpoint` the model describes, built entirely from the stored
/// record.
///
/// `ServerEndpoint` is reported only once the endpoint is `PROVISIONED` — there is no
/// address to connect to while it is still being provisioned. `LicenseServers` is left
/// absent rather than invented: this emulator provisions no license-server hosts, so
/// there are none to report. `StatusMessage` is only what the record carries.
pub fn license_server_endpoint(record: &LicenseServerEndpointRecord) -> LicenseServerEndpoint {
    let provisioned = record.provisioning_status == PROVISIONING_STATUS_PROVISIONED;
    LicenseServerEndpoint {
        identity_provider_arn: Some(record.identity_provider_arn.clone()),
        server_type: Some(record.server_type.clone()),
        server_endpoint: provisioned.then(|| ServerEndpoint {
            endpoint: Some(record.endpoint.clone()),
        }),
        status_message: record.status_message.clone(),
        license_server_endpoint_id: Some(record.id.clone()),
        license_server_endpoint_arn: Some(record.arn.clone()),
        license_server_endpoint_provisioning_status: Some(record.provisioning_status.clone()),
        license_servers: None,
        creation_time: Some(::aws_smithy_types::DateTime::from(record.created_at)),
    }
}

/// One page of `items`, plus the token that reaches the next one.
///
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning. A page size below 1 would make no progress, so it is
/// refused too.
pub fn paginate<T: Clone>(
    items: &[T],
    kind: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), StateError> {
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token)?,
        None => 0,
    };
    if offset > items.len() {
        return Err(StateError::Validation(
            "NextToken does not refer to a position in this list".to_string(),
        ));
    }
    let page_size = match max_results {
        Some(max) if max < 1 => {
            return Err(StateError::Validation(format!(
                "MaxResults must be at least 1, got {max}"
            )))
        }
        Some(max) => max as usize,
        None => items.len(),
    };
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
    Ok((page, token))
}

/// This service's opaque pagination cursor: the list it belongs to and the offset it
/// resumes at.
///
/// Hex is an encoding, not a secret — a client can decode one. What makes a cursor
/// trustworthy here is the `kind` check in [`decode_page_token`]: a token is only honoured
/// by the list that minted it, so a forged or stale one is refused rather than applied to
/// a different list.
fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}

/// Decode a cursor minted by [`encode_page_token`] for the same list.
///
/// The offset is split off the END, so a list identity may itself contain `:`.
fn decode_page_token(kind: &str, token: &str) -> Result<usize, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    let decoded = String::from_utf8(decoded).map_err(|_| invalid())?;
    let (token_kind, offset) = decoded.rsplit_once(':').ok_or_else(invalid)?;
    if token_kind != kind {
        return Err(invalid());
    }
    offset.parse::<usize>().map_err(|_| invalid())
}

/// A deterministic rendering of a filter set, for the list identity a pagination cursor
/// is minted against: a cursor is a position in ONE list, so it must not be accepted for
/// a list selected by different filters.
///
/// Every component is LENGTH-PREFIXED, so no two different filter sets can render to the
/// same signature — a separator appearing inside an attribute, operation or value cannot
/// make one set look like another.
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        for part in [
            filter.attribute.as_deref().unwrap_or_default(),
            filter.operation.as_deref().unwrap_or_default(),
            filter.value.as_deref().unwrap_or_default(),
        ] {
            signature.push_str(&format!("{}.{part}", part.len()));
        }
    }
    signature
}

/// True when `attributes` satisfies every filter in `filters`.
///
/// `Filter.Attribute` names one of the attributes the calling operation's model
/// documents as filterable (`Product`/`DirectoryId` for `ListIdentityProviders`,
/// `Status`/`Username`/`Domain` for `ListProductSubscriptions`, `IdentityProviderArn`
/// for `ListLicenseServerEndpoints`): the caller passes those
/// as `(name, value)` pairs, matched EXACTLY as the documentation spells them, so a typo
/// is refused rather than silently honoured. `Filter.Operation` is the kind of search —
/// the model documents it only as "the type of search (For example, eq, geq, leq)", so
/// the comparisons that are meaningful for these string attributes are accepted under
/// either the documented lowercase spelling or its PascalCase name, and anything else is
/// refused; an absent operation is equality.
///
/// Whether a filter is WELL FORMED does not depend on what the store holds, so
/// [`validate_filters`] is what a list operation calls first, before it looks at a single
/// record: otherwise an empty (or fully filtered-out) store would answer 200 for a
/// request this service refuses.
pub fn filters_match(
    filters: &[Filter],
    attributes: &[(&str, Option<&str>)],
) -> Result<bool, StateError> {
    for filter in filters {
        let (index, operation) = parse_filter(filter, &attribute_names(attributes))?;
        let actual = attributes[index].1.unwrap_or_default();
        let wanted = filter.value.as_deref().unwrap_or_default();
        let matched = match operation {
            FilterOperation::Equals => actual == wanted,
            FilterOperation::NotEquals => actual != wanted,
            FilterOperation::Contains => actual.contains(wanted),
            FilterOperation::BeginsWith => actual.starts_with(wanted),
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Refuse a filter set that names an attribute this operation does not expose, or a
/// comparison this service does not implement — independently of the records in state.
pub fn validate_filters(filters: &[Filter], attribute_names: &[&str]) -> Result<(), StateError> {
    for filter in filters {
        parse_filter(filter, attribute_names)?;
    }
    Ok(())
}

/// The comparison a `Filter.Operation` names.
#[derive(Debug, Clone, Copy)]
enum FilterOperation {
    Equals,
    NotEquals,
    Contains,
    BeginsWith,
}

/// The names of the attributes a list operation exposes, in the order it passed them.
fn attribute_names<'a>(attributes: &[(&'a str, Option<&str>)]) -> Vec<&'a str> {
    attributes.iter().map(|(name, _)| *name).collect()
}

/// Resolve one filter against the attributes a list operation exposes: which attribute it
/// names (by index) and which comparison it asks for.
fn parse_filter(
    filter: &Filter,
    attribute_names: &[&str],
) -> Result<(usize, FilterOperation), StateError> {
    let Some(attribute) = filter.attribute.as_deref() else {
        return Err(StateError::Validation(
            "Filter.Attribute is required".to_string(),
        ));
    };
    let index = attribute_names
        .iter()
        .position(|name| *name == attribute)
        .ok_or_else(|| {
            StateError::Validation(format!(
                "Filter.Attribute {attribute} is not filterable; supported attributes: {}",
                attribute_names.join(", ")
            ))
        })?;
    let operation = filter.operation.as_deref().unwrap_or("Equals");
    let operation = match operation.to_ascii_lowercase().as_str() {
        "equals" | "eq" => FilterOperation::Equals,
        "notequals" | "ne" => FilterOperation::NotEquals,
        "contains" => FilterOperation::Contains,
        "beginswith" => FilterOperation::BeginsWith,
        _ => {
            return Err(StateError::Validation(format!(
                "Filter.Operation {operation} is not supported; supported operations: \
                 Equals, NotEquals, Contains, BeginsWith"
            )))
        }
    };
    Ok((index, operation))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>)
}
