// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-vpclattice-mercurycontrolplane service.
//!
//! One store holds the service-network / service / service-association / listener /
//! listener-rule slice, and the
//! helpers every handler in `src/ops/` shares live here too: identifier resolution
//! (bare id or ARN), id and ARN minting, the request's region, and the clock-owed
//! `CREATE_IN_PROGRESS -> ACTIVE` span. Handlers reach the store through
//! [`service_state`]; nothing in `src/ops/` keeps a second copy of a resource.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// This service's ARN namespace and sigv4 signing name (OPERATIONS.md, "Wire protocol").
pub const ARN_SERVICE: &str = "vpc-lattice";

/// The region for a request that carries no SigV4 credential scope (an unsigned call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN resource kind of a service network: `.../servicenetwork/sn-…`.
pub const KIND_SERVICE_NETWORK: &str = "servicenetwork";
/// The ARN resource kind of a service: `.../service/svc-…`.
pub const KIND_SERVICE: &str = "service";
/// The ARN resource kind of a service-network/service association.
pub const KIND_SERVICE_ASSOCIATION: &str = "servicenetworkserviceassociation";
/// The ARN resource kind of a listener. A listener ARN nests under its service's:
/// `…:service/svc-…/listener/listener-…` (`ListenerArn`'s modelled pattern).
pub const KIND_LISTENER: &str = "listener";
/// The ARN resource kind of a listener rule. A rule ARN nests under its listener's:
/// `…:service/svc-…/listener/listener-…/rule/rule-…` (`RuleArn`'s modelled pattern).
pub const KIND_RULE: &str = "rule";

/// A resource kind this service's error bodies name.
///
/// One kind has more than one spelling and they are NOT interchangeable: the ARN segment
/// (`servicenetwork`, used in an ARN and in the exceptions' `resourceType` member) is an
/// internal, lowercase, delimiter-free key, while the token AWS's error MESSAGES name the
/// kind by is its CamelCase resource name (`ServiceNetwork`). Keeping both on one type is
/// what stops a handler reaching for the ARN segment where the message token belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// A service network (`sn-…`).
    ServiceNetwork,
    /// A service (`svc-…`).
    Service,
    /// A service-network/service association (`snsa-…`).
    ServiceAssociation,
    /// A listener of a service (`listener-…`).
    Listener,
    /// A rule of a listener (`rule-…`).
    Rule,
}

impl ResourceKind {
    /// This kind's ARN resource segment, which is also the `resourceType` its error
    /// bodies carry.
    pub fn arn_kind(self) -> &'static str {
        match self {
            Self::ServiceNetwork => KIND_SERVICE_NETWORK,
            Self::Service => KIND_SERVICE,
            Self::ServiceAssociation => KIND_SERVICE_ASSOCIATION,
            Self::Listener => KIND_LISTENER,
            Self::Rule => KIND_RULE,
        }
    }

    /// The token AWS's error messages name this kind by.
    ///
    /// Source: recorded vpc-lattice not-found bodies, which spell the kind as its
    /// CamelCase resource name — `ServiceNetwork` for a service network, `Service` for a
    /// service, and `Association` (not the full association shape name) for a
    /// service-network/service association. No listener not-found body is recorded, so a
    /// listener takes the same rule the other three follow: the Smithy resource name,
    /// `com.amazonaws.vpclattice#Listener`. A listener RULE follows it too, over
    /// `com.amazonaws.vpclattice#Rule`.
    pub fn error_label(self) -> &'static str {
        match self {
            Self::ServiceNetwork => "ServiceNetwork",
            Self::Service => "Service",
            Self::ServiceAssociation => "Association",
            Self::Listener => "Listener",
            Self::Rule => "Rule",
        }
    }

    /// AWS's `ResourceNotFoundException.message` construction:
    /// `"<ResourceType> <identifier> cannot be found."`.
    ///
    /// `identifier` is the identifier the CALLER submitted — whichever modelled form it
    /// sent (bare id or ARN) — so the message names back the thing the request named, and
    /// is reportable even when nothing resolved. The model types `message` as a required
    /// String and states no text, so the construction (kind token plus submitted
    /// identifier plus "cannot be found.") is what the recordings fix, not one string.
    pub fn not_found_message(self, identifier: &str) -> String {
        format!("{} {identifier} cannot be found.", self.error_label())
    }
}

/// The id prefix of a service network (`^sn-[0-9a-z]{17}$`).
pub const PREFIX_SERVICE_NETWORK: &str = "sn-";
/// The id prefix of a service (`^svc-[0-9a-z]{17}$`).
pub const PREFIX_SERVICE: &str = "svc-";
/// The id prefix of a service-network/service association (`^snsa-[0-9a-z]{17}$`).
pub const PREFIX_SERVICE_ASSOCIATION: &str = "snsa-";
/// The id prefix of a listener (`^listener-[0-9a-z]{17}$`).
pub const PREFIX_LISTENER: &str = "listener-";
/// The id prefix of a listener rule (`^rule-[0-9a-z]{17}$`).
pub const PREFIX_RULE: &str = "rule-";

/// Every modelled id of this service is its prefix plus 17 `[0-9a-z]` characters.
const ID_SUFFIX_LEN: usize = 17;

/// The most items one page of a list operation carries.
///
/// This is the maximum the model allows `maxResults` (`range 1..100`), so it is also
/// the right ceiling for a request that sends no `maxResults`: a single list call can
/// never be made to serialise the whole store, and `nextToken` pages the rest.
pub const MAX_PAGE_SIZE: usize = 100;

/// The `idleTimeoutSeconds` a service gets when its create omits the member.
///
/// From that member's own documentation: "The valid range is 60 to 600 seconds. If you
/// don't specify a value, the default is 60 seconds."
pub const DEFAULT_IDLE_TIMEOUT_SECONDS: i32 = 60;

/// `sharingConfig.enabled` a service network gets when its create sends no
/// `sharingConfig` (or sends one that omits `enabled`).
///
/// `sharingConfig` is a member the service fills in itself: `CreateServiceNetwork` and
/// `GetServiceNetwork` both report it for a create that never mentioned it, so the
/// stored value is a real setting and not "absent". The recorded AWS answers for such a
/// create report `{"enabled": true}` — sharing enabled is what a service network has
/// always had — so that is the default here.
pub const DEFAULT_SHARING_ENABLED: bool = true;

/// The `@length` ceiling the model puts on `TagMap` (`len ..200`).
///
/// Checked in the handlers: a map's cardinality is one of the modelled constraints the
/// boundary does not refuse for this service's `tags` member, and a caller must not be
/// able to store more tags than AWS accepts.
pub const MAX_TAGS: usize = 200;

/// How many service networks this emulator will hold for the account.
///
/// The real service caps service networks per account per Region with a service quota,
/// and `CreateServiceNetwork` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding it. The number here is NOT a fidelity claim about the published quota
/// (which is smaller and adjustable): it is the emulator's own ceiling, set generously
/// so no realistic workflow meets it, so that a caller looping on create cannot grow
/// this process's memory without bound.
pub const MAX_SERVICE_NETWORKS: usize = 1_000;

/// How many services this emulator will hold for the account. Same reasoning, and
/// `CreateService` models the same `ServiceQuotaExceededException`.
pub const MAX_SERVICES: usize = 1_000;

/// How many service-network/service associations this emulator will hold. Same
/// reasoning, and `CreateServiceNetworkServiceAssociation` models the same
/// `ServiceQuotaExceededException`.
pub const MAX_SERVICE_ASSOCIATIONS: usize = 1_000;

/// How many listeners this emulator will hold for one service. Same reasoning as the
/// ceilings above, and `CreateListener` models the same `ServiceQuotaExceededException`.
pub const MAX_LISTENERS_PER_SERVICE: usize = 100;

/// How many rules this emulator will hold for one listener. Same reasoning as the
/// ceilings above, and `CreateRule` models the same `ServiceQuotaExceededException`.
/// Not a fidelity claim about the published per-listener rule quota: it is this
/// emulator's own ceiling, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_RULES_PER_LISTENER: usize = 100;

/// The listener port a create gets when it omits `port`, by protocol.
///
/// From the `port` member's own documentation: "The listener port. You can specify a
/// value from 1 to 65535. For HTTP, the default is 80. For HTTPS, the default is 443."
/// No default is documented for any other protocol, so [`default_port_for`] answers
/// `None` there and the handler refuses a create that sends no port rather than
/// inventing one.
pub const DEFAULT_PORT_HTTP: i32 = 80;
/// The documented default port of an `HTTPS` listener; see [`DEFAULT_PORT_HTTP`].
pub const DEFAULT_PORT_HTTPS: i32 = 443;

/// The weight a forward action's only target group gets when the caller sends none.
///
/// From `WeightedTargetGroup`'s `weight` documentation: "Only required if you specify
/// multiple target groups for a forward action. … If there's only one target group
/// specified, then the default value is 100."
pub const DEFAULT_SINGLE_TARGET_GROUP_WEIGHT: i32 = 100;

/// The HTTP status codes a fixed-response action may carry.
///
/// From `FixedResponseAction`'s `statusCode` documentation: "The HTTP response code.
/// Only `404` and `500` status codes are supported." The modelled `@range(100, 599)` is
/// wider than that, so this narrower catalog is enforced in the handlers.
pub const FIXED_RESPONSE_STATUS_CODES: &[i32] = &[404, 500];

/// How long after its create call a resource still reports `CREATE_IN_PROGRESS`.
///
/// VPC Lattice creates are asynchronous: the model gives a service and an association a
/// non-terminal `CREATE_IN_PROGRESS` status and the resource reaches `ACTIVE` on its
/// own. AWS documents no duration for it, so this is the emulator's CHOSEN span, inside
/// the 250–750 ms window this project fixes for a transition the clock owes. It is
/// never zero, so a caller that reads straight back still observes the transient
/// status; nothing sleeps for it — the due instant is stored and compared on read.
pub const CREATE_TRANSITION: Duration = Duration::from_millis(500);

/// The lowercase base-36 alphabet the modelled id patterns allow (`[0-9a-z]`).
const BASE36_LOWER: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// The uppercase base-36 alphabet a Route 53 hosted-zone id is drawn from (`[0-9A-Z]`).
const BASE36_UPPER: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// A service network: the logical boundary services and VPCs associate with.
#[derive(Debug, Clone)]
pub struct ServiceNetwork {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// `AuthType`: `NONE` unless the create call asked for `AWS_IAM` (the model
    /// documents `NONE` as the default).
    pub auth_type: String,
    /// `sharingConfig.enabled` as the create call set it, or
    /// [`DEFAULT_SHARING_ENABLED`] when it sent no value for it. Never "absent": the
    /// service reports this member on every response that models it.
    pub sharing_enabled: bool,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

/// A VPC Lattice service. Named `LatticeService` so it does not read as this crate's
/// emulated `Service`.
#[derive(Debug, Clone)]
pub struct LatticeService {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub custom_domain_name: Option<String>,
    pub certificate_arn: Option<String>,
    pub auth_type: String,
    /// What the caller sent, or [`DEFAULT_IDLE_TIMEOUT_SECONDS`] — the value the
    /// member's own documentation gives a create that omits it.
    pub idle_timeout_seconds: i32,
    pub dns_domain_name: String,
    pub dns_hosted_zone_id: String,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// The instant the clock owes this service its `ACTIVE` status.
    pub active_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

impl LatticeService {
    /// `ServiceStatus` at `now`: the transient create status until the stored due
    /// instant passes, `ACTIVE` from then on. A pure function of stored state and the
    /// clock, so reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            "ACTIVE"
        } else {
            "CREATE_IN_PROGRESS"
        }
    }

    /// This service's DNS entry, as persisted.
    pub fn dns_entry(&self) -> (String, String) {
        (self.dns_domain_name.clone(), self.dns_hosted_zone_id.clone())
    }
}

/// An association between one service network and one service.
#[derive(Debug, Clone)]
pub struct ServiceNetworkServiceAssociation {
    pub id: String,
    pub arn: String,
    pub created_by: String,
    pub created_at: SystemTime,
    /// The instant the clock owes this association its `ACTIVE` status.
    pub active_at: SystemTime,
    pub service_id: String,
    pub service_arn: String,
    pub service_name: String,
    pub service_network_id: String,
    pub service_network_arn: String,
    pub service_network_name: String,
    pub dns_domain_name: String,
    pub dns_hosted_zone_id: String,
    pub custom_domain_name: Option<String>,
    pub tags: HashMap<String, String>,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

impl ServiceNetworkServiceAssociation {
    /// `ServiceNetworkServiceAssociationStatus` at `now`, on the same clock-owed rule
    /// as [`LatticeService::status`].
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            "ACTIVE"
        } else {
            "CREATE_IN_PROGRESS"
        }
    }
}

/// A listener of one service: the process that checks for connection requests to it.
///
/// The model gives a listener no status member — AWS answers a created listener with
/// nothing non-terminal — so there is no clock-owed transition here, only the
/// timestamps `GetListener` and `ListListeners` report.
#[derive(Debug, Clone)]
pub struct Listener {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// `ListenerProtocol`, exactly as the create sent it (the model constrains it to
    /// `LISTENER_PROTOCOL_VALUES`).
    pub protocol: String,
    /// The port the create sent, or the protocol's documented default; see
    /// [`default_port_for`].
    pub port: i32,
    pub service_id: String,
    pub service_arn: String,
    /// The default rule's action, stored as the modelled union after
    /// [`normalized_rule_action`] filled in the documented weight default.
    pub default_action: crate::ops::types::RuleAction,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

/// A listener rule: one priority, one match and one action under a listener.
///
/// The model gives a rule no status member, so there is no clock-owed transition here —
/// only the timestamps `GetRule` and `ListRules` report.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// The listener this rule belongs to; a rule is only reachable through it.
    pub listener_id: String,
    /// The listener's service, so a read through the wrong service is a 404.
    pub service_id: String,
    /// `RulePriority`. "Each rule for a specific listener must have a unique priority"
    /// (the `priority` member's own documentation), so this is unique per `listener_id`.
    pub priority: i32,
    /// The modelled `RuleMatch` exactly as the create sent it.
    pub r#match: crate::ops::types::RuleMatch,
    /// The rule's action, stored as the modelled union after [`normalized_rule_action`]
    /// filled in the documented weight default.
    pub action: crate::ops::types::RuleAction,
    /// `isDefault`: "Indicates whether this is the default rule." A listener's default
    /// rule is the one `CreateListener` carries as `defaultAction`; every rule
    /// `CreateRule` mints is an additional rule, so this is false for them.
    pub is_default: bool,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

/// The default port of a listener speaking `protocol`, or `None` where the `port`
/// member documents no default for it.
pub fn default_port_for(protocol: &str) -> Option<i32> {
    match protocol {
        "HTTP" => Some(DEFAULT_PORT_HTTP),
        "HTTPS" => Some(DEFAULT_PORT_HTTPS),
        _ => None,
    }
}

/// Refuse a rule action outside a constraint its own documentation states, with the
/// offending member path and the reason; `Ok(())` when it satisfies them.
///
/// The modelled traits the boundary already enforces (`targetGroups`' `@length(1, 10)`,
/// `weight`'s `@range(0, 999)`, `statusCode`'s `@range(100, 599)`) are not re-checked
/// here; what is checked is the narrower documented catalog of fixed-response status
/// codes ([`FIXED_RESPONSE_STATUS_CODES`]), which no modelled trait carries.
pub fn check_rule_action(
    member: &str,
    action: &crate::ops::types::RuleAction,
) -> Result<(), (String, String)> {
    match action {
        crate::ops::types::RuleAction::fixedResponse(fixed) => {
            if !FIXED_RESPONSE_STATUS_CODES.contains(&fixed.status_code) {
                let supported = FIXED_RESPONSE_STATUS_CODES
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err((
                    format!("{member}.fixedResponse.statusCode"),
                    format!(
                        "must be one of the supported status codes {supported}, got {}",
                        fixed.status_code
                    ),
                ));
            }
            Ok(())
        }
        crate::ops::types::RuleAction::Forward(_) => Ok(()),
    }
}

/// The action as it is PERSISTED: the caller's action with every value its
/// documentation defaults filled in.
///
/// Today that is one default — a forward action whose single target group carries no
/// `weight` gets [`DEFAULT_SINGLE_TARGET_GROUP_WEIGHT`]. A weight omitted beside other
/// target groups is left absent: the member documents a default only for the
/// single-target-group case.
pub fn normalized_rule_action(
    action: &crate::ops::types::RuleAction,
) -> crate::ops::types::RuleAction {
    let mut action = action.clone();
    if let crate::ops::types::RuleAction::Forward(forward) = &mut action {
        if forward.target_groups.len() == 1 {
            if let Some(only) = forward.target_groups.first_mut() {
                only.weight = Some(only.weight.unwrap_or(DEFAULT_SINGLE_TARGET_GROUP_WEIGHT));
            }
        }
    }
    action
}

/// The most `@idempotencyToken` -> resource-id entries this service remembers.
///
/// Tokens are caller-supplied, so the map is caller-driven growth that holds no resource
/// of its own: an entry naming a deleted resource is dropped with it
/// ([`IdempotencyTokens::forget_resource`]), and beyond this many live entries the oldest
/// is evicted ([`IdempotencyTokens::record`]). An evicted token is indistinguishable from
/// one whose idempotency window has passed: the next create carrying it mints a new
/// resource.
const MAX_IDEMPOTENCY_TOKENS: usize = 1024;

/// A bounded `@idempotencyToken` -> created-resource-id store.
///
/// Insertion-ordered and capped at [`MAX_IDEMPOTENCY_TOKENS`]: recording the
/// (cap + 1)-th token forgets the oldest one rather than growing the map.
#[derive(Debug, Default)]
pub struct IdempotencyTokens {
    by_key: HashMap<String, String>,
    oldest_first: std::collections::VecDeque<String>,
}

impl IdempotencyTokens {
    /// The resource id a previous call recorded under `key`, if it is still remembered.
    pub fn get(&self, key: &str) -> Option<&String> {
        self.by_key.get(key)
    }

    /// Remember that `key` created `resource_id`, evicting the oldest entries to stay
    /// within [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record(&mut self, key: String, resource_id: String) {
        if self.by_key.insert(key.clone(), resource_id).is_none() {
            self.oldest_first.push_back(key);
        }
        while self.oldest_first.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many tokens are remembered right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether no token is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Forget every token that names `resource_id` — called when that resource is
    /// deleted, so a token record never outlives the resource it created.
    pub fn forget_resource(&mut self, resource_id: &str) {
        self.by_key.retain(|_, created| created != resource_id);
        let live = &self.by_key;
        self.oldest_first.retain(|key| live.contains_key(key));
    }
}

/// Per-service state for com-amazonaws-vpclattice-mercurycontrolplane. A handler reaches it through [`service_state`].
///
/// Every map here is bounded. The three resource maps are capped at
/// [`MAX_SERVICE_NETWORKS`], [`MAX_SERVICES`] and [`MAX_SERVICE_ASSOCIATIONS`] — a create
/// that would exceed its ceiling is refused with the modelled
/// `ServiceQuotaExceededException` — and `name_index` holds exactly one key per stored
/// resource, released by the same delete that removes the record. The token store is
/// capped and evicted on its own terms; see [`IdempotencyTokens`].
#[derive(Debug, Default)]
pub struct ComAmazonawsVpclatticeMercurycontrolplaneState {
    /// Keyed by `sn-…` id. At most [`MAX_SERVICE_NETWORKS`] entries.
    pub service_networks: HashMap<String, ServiceNetwork>,
    /// Keyed by `svc-…` id. At most [`MAX_SERVICES`] entries.
    pub services: HashMap<String, LatticeService>,
    /// Keyed by `snsa-…` id. At most [`MAX_SERVICE_ASSOCIATIONS`] entries.
    pub service_associations: HashMap<String, ServiceNetworkServiceAssociation>,
    /// Keyed by `listener-…` id. At most [`MAX_LISTENERS_PER_SERVICE`] entries per
    /// service, and every listener of a deleted service goes with it
    /// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::remove_listeners_of`]).
    pub listeners: HashMap<String, Listener>,
    /// Keyed by `rule-…` id. At most [`MAX_RULES_PER_LISTENER`] entries per listener,
    /// and every rule of a deleted listener goes with it
    /// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::remove_rules_of`]).
    pub rules: HashMap<String, Rule>,
    /// `<ARN resource kind>/<name>` -> id. A name must be unique to the account, so a
    /// second create under a taken name is a `ConflictException`. One key per stored
    /// resource, so the resource ceilings bound this map too.
    pub name_index: HashMap<String, String>,
    /// `<operation>/<clientToken>` -> the id the first call created, so a retry with the
    /// same `@idempotencyToken` returns that resource instead of creating a second one.
    pub idempotency_tokens: IdempotencyTokens,
    /// Hands out [`ServiceNetwork::sequence`] and its siblings: the creation order a
    /// listing pages by. Monotonic, so a deleted resource's place is never reused.
    next_sequence: u64,
}

impl ComAmazonawsVpclatticeMercurycontrolplaneState {
    /// The creation-order number for the resource being created now.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// How many services are associated with `service_network_id`, counted from the
    /// association records at read time rather than from a stored counter.
    pub fn associated_service_count(&self, service_network_id: &str) -> i64 {
        i64::try_from(
            self.service_associations
                .values()
                .filter(|assoc| assoc.service_network_id == service_network_id)
                .count(),
        )
        .unwrap_or(i64::MAX)
    }

    /// How many service networks `service_id` is associated with, counted the same way.
    pub fn associated_network_count(&self, service_id: &str) -> i64 {
        i64::try_from(
            self.service_associations
                .values()
                .filter(|assoc| assoc.service_id == service_id)
                .count(),
        )
        .unwrap_or(i64::MAX)
    }

    /// How many listeners `service_id` holds, counted from the listener records.
    pub fn listener_count(&self, service_id: &str) -> usize {
        self.listeners
            .values()
            .filter(|listener| listener.service_id == service_id)
            .count()
    }

    /// How many rules `listener_id` holds, counted from the rule records.
    pub fn rule_count(&self, listener_id: &str) -> usize {
        self.rules
            .values()
            .filter(|rule| rule.listener_id == listener_id)
            .count()
    }

    /// The rule of `listener_id` that already holds `priority`, if there is one.
    ///
    /// "Each rule for a specific listener must have a unique priority" (the `priority`
    /// member's own documentation), so a create or update that would take a priority
    /// another rule of the same listener holds is a `ConflictException`.
    pub fn rule_with_priority(&self, listener_id: &str, priority: i32) -> Option<&Rule> {
        self.rules
            .values()
            .find(|rule| rule.listener_id == listener_id && rule.priority == priority)
    }

    /// Drop every rule of `listener_id`, releasing its name and token records.
    ///
    /// A rule is a resource OF its listener, so deleting the listener (or the service
    /// that holds it) deletes its rules: `DeleteService` documents that "listener rules"
    /// go with the service.
    pub fn remove_rules_of(&mut self, listener_id: &str) {
        let doomed: Vec<Rule> = self
            .rules
            .values()
            .filter(|rule| rule.listener_id == listener_id)
            .cloned()
            .collect();
        for rule in doomed {
            self.rules.remove(&rule.id);
            self.name_index
                .remove(&rule_name_key(listener_id, &rule.name));
            self.idempotency_tokens.forget_resource(&rule.id);
        }
    }

    /// Drop every listener of `service_id`, releasing its name and token records.
    ///
    /// `DeleteService`'s own documentation: "If you delete a service, all resources
    /// related to the service, such as the resource policy, auth policy, listeners,
    /// listener rules, and access log subscriptions, are also deleted."
    pub fn remove_listeners_of(&mut self, service_id: &str) {
        let doomed: Vec<Listener> = self
            .listeners
            .values()
            .filter(|listener| listener.service_id == service_id)
            .cloned()
            .collect();
        for listener in doomed {
            self.remove_rules_of(&listener.id);
            self.listeners.remove(&listener.id);
            self.name_index
                .remove(&listener_name_key(service_id, &listener.name));
            self.idempotency_tokens.forget_resource(&listener.id);
        }
    }

    /// The association of this exact (service network, service) pair, if there is one.
    pub fn association_of(
        &self,
        service_network_id: &str,
        service_id: &str,
    ) -> Option<&ServiceNetworkServiceAssociation> {
        self.service_associations.values().find(|assoc| {
            assoc.service_network_id == service_network_id && assoc.service_id == service_id
        })
    }
}

/// The key [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] is read with.
pub fn name_key(kind: &str, name: &str) -> String {
    format!("{kind}/{name}")
}

/// The [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] key of a listener
/// name. A listener name is unique within its SERVICE, not within the account, so the
/// owning service's id is part of the key.
pub fn listener_name_key(service_id: &str, name: &str) -> String {
    name_key(&format!("{KIND_SERVICE}/{service_id}/{KIND_LISTENER}"), name)
}

/// The [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] key of a rule
/// name. "The name must be unique within the listener" (`CreateRule`'s `name` member),
/// so the owning listener's id is part of the key.
pub fn rule_name_key(listener_id: &str, name: &str) -> String {
    name_key(&format!("{KIND_LISTENER}/{listener_id}/{KIND_RULE}"), name)
}

/// A `nextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Encode the resume cursor that follows the item with creation-order number `sequence`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(1, 2048)`. Naming the ORDER rather than an id keeps the cursor usable when
/// the item the page ended on is deleted between pages: the next page resumes at the
/// first surviving item after that point, instead of refusing a token this service
/// itself handed out.
pub fn encode_cursor(sequence: u64) -> String {
    ::hex::encode(format!("after-sequence:{sequence}"))
}

/// Decode a cursor minted by [`encode_cursor`]; no cursor starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<u64, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(0);
    };
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix("after-sequence:")
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .ok_or(MalformedCursor)
}

/// The key [`ComAmazonawsVpclatticeMercurycontrolplaneState::idempotency_tokens`] is
/// read with: a token is scoped to the operation that minted it.
pub fn token_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// The region this request was signed for, from its SigV4 credential scope;
/// [`DEFAULT_REGION`] when it carries none.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The emulator's account id, the `createdBy` and ARN account of every resource here.
pub fn account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// `arn:aws:vpc-lattice:<region>:<account>:<kind>/<id>`.
pub fn arn_for(region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{kind}/{id}"))
}

/// A listener's ARN, which nests under its service's:
/// `arn:aws:vpc-lattice:<region>:<account>:service/<service id>/listener/<listener id>`
/// (`ListenerArn`'s modelled pattern).
pub fn listener_arn(region: &str, service_id: &str, listener_id: &str) -> String {
    arn_for(
        region,
        &format!("{KIND_SERVICE}/{service_id}/{KIND_LISTENER}"),
        listener_id,
    )
}

/// A rule's ARN, which nests under its listener's:
/// `arn:aws:vpc-lattice:<region>:<account>:service/<service id>/listener/<listener
/// id>/rule/<rule id>` (`RuleArn`'s modelled pattern).
pub fn rule_arn(region: &str, service_id: &str, listener_id: &str, rule_id: &str) -> String {
    arn_for(
        region,
        &format!("{KIND_SERVICE}/{service_id}/{KIND_LISTENER}/{listener_id}/{KIND_RULE}"),
        rule_id,
    )
}

/// Mint a fresh resource id: `prefix` plus 17 `[0-9a-z]` characters, as the modelled
/// id patterns require.
pub fn mint_id(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        encode_base36(fresh_entropy(), ID_SUFFIX_LEN, BASE36_LOWER)
    )
}

/// Mint the Route 53 hosted-zone id of a service's DNS entry: `Z` plus 19 `[0-9A-Z]`
/// characters, the shape Route 53 gives a hosted zone.
pub fn mint_hosted_zone_id() -> String {
    format!("Z{}", encode_base36(fresh_entropy(), 19, BASE36_UPPER))
}

/// The generated domain name of a service's DNS entry:
/// `<name>-<id suffix>.<account segment>.vpc-lattice-svcs.<region>.on.aws`, the form
/// VPC Lattice hands out. The account segment is derived from the account id, so every
/// service of one account shares it and it is stable across reads.
pub fn service_domain_name(name: &str, service_id: &str, region: &str) -> String {
    let suffix = service_id
        .strip_prefix(PREFIX_SERVICE)
        .unwrap_or(service_id);
    format!(
        "{name}-{suffix}.{}.vpc-lattice-svcs.{region}.on.aws",
        account_segment()
    )
}

/// The 7-hex account segment of a lattice domain name, derived from the account id.
fn account_segment() -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(account_id().as_bytes());
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..7]
        .to_string()
}

/// Entropy for one minted id.
///
/// Mixes this process's OS-seeded [`std::collections::hash_map::RandomState`] keys with
/// the clock and a process-wide counter, then hashes them, so ids are unique within a
/// run and unguessable across runs without any id ever being derived from the request.
fn fresh_entropy() -> u128 {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);

    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let os_seeded = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let digest = Sha256::digest(format!("{nanos}:{minted}:{os_seeded}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(bytes)
}

/// `value` rendered in `alphabet`, exactly `len` characters, most significant first.
fn encode_base36(mut value: u128, len: usize, alphabet: &[u8; 36]) -> String {
    let mut out = vec![alphabet[0]; len];
    for slot in out.iter_mut().rev() {
        *slot = alphabet[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(out).expect("the base-36 alphabets are ASCII")
}

/// The resource id named by an identifier member, accepting both forms the model allows:
/// the bare id, or the resource's ARN (`arn:…:<kind>/<id>`).
///
/// `None` when the identifier names no id of `prefix`'s kind — which every handler
/// answers with `ResourceNotFoundException`, never with a lookup against another kind.
pub fn id_from_identifier(identifier: &str, prefix: &str) -> Option<String> {
    let tail = identifier
        .rsplit_once('/')
        .map_or(identifier, |(_, tail)| tail);
    let suffix = tail.strip_prefix(prefix)?;
    (suffix.len() == ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))
    .then(|| tail.to_string())
}

/// Refuse a resource name the modelled name pattern forbids, with the text that names
/// the broken constraint; `Ok(())` when the name satisfies it.
///
/// The rule the `ServiceName` / `ServiceNetworkName` patterns state: lowercase letters,
/// digits and hyphens only, no leading or trailing hyphen, no two adjacent hyphens,
/// and — where the resource's ids carry one — not beginning with that id prefix. The
/// length bounds are a separate `@length` trait the framework already enforces.
///
/// This has to be enforced in the handler: those patterns are written with negative
/// lookahead, which the regex engine behind `@pattern` cannot compile, and a pattern
/// that does not compile is treated as matching, so the boundary lets the name through.
pub fn check_resource_name(name: &str, reserved_prefix: Option<&str>) -> Result<(), String> {
    if reserved_prefix.is_some_and(|prefix| name.starts_with(prefix)) {
        return Err(format!(
            "must not begin with the reserved prefix {}",
            reserved_prefix.unwrap_or_default()
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("may contain only lowercase letters, digits and hyphens".to_string());
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("must not begin or end with a hyphen".to_string());
    }
    if name.contains("--") {
        return Err("must not contain two adjacent hyphens".to_string());
    }
    Ok(())
}

/// Refuse a `tags` map larger than the modelled `TagMap` ceiling, with the text naming
/// the constraint; `Ok(())` when it fits.
pub fn check_tag_count(tags: Option<&HashMap<String, String>>) -> Result<(), String> {
    let count = tags.map_or(0, HashMap::len);
    if count > MAX_TAGS {
        return Err(format!(
            "must carry at most {MAX_TAGS} tags, got {count}"
        ));
    }
    Ok(())
}

/// A stored instant as the modelled timestamp.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsVpclatticeMercurycontrolplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsVpclatticeMercurycontrolplaneState>)
}
