// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53resolver-route53resolver service.
//!
//! Besides the state struct this module owns the small pieces the Resolver
//! endpoint, Resolver rule, Resolver-on-Outpost and DNS Firewall domain-list
//! operations share, so no two
//! handlers grow their own copy: id minting, the RFC 3339 clock read, the
//! clock-owed status transition, the opaque pagination cursor the `@paginated`
//! list operations mint and verify, the record -> modelled-shape projections,
//! and the exception bodies the operations report a missing resource with.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

// The account's own AWS Outposts inventory, which `account_holds_outpost`
// resolves an `OutpostArn` against: the Outposts live in that service's store,
// so they are read through its own accessors.
use ::service_com_amazonaws_outposts_outpostsolafservice::state as outposts;

use crate::ops::types::{
    AccessDeniedException, ConflictException, FirewallConfig, FirewallDomainList,
    FirewallDomainListMetadata,
    FirewallRule, FirewallRuleGroup, FirewallRuleGroupMetadata,
    InvalidParameterException,
    InvalidPolicyDocument, InvalidRequestException, IpAddressResponse, IpAddressUpdate,
    LimitExceededException, OutpostResolver, ResolverConfig, ResolverDnssecConfig, ResolverEndpoint,
    ResolverQueryLogConfig, ResolverQueryLogConfigAssociation, ResolverRule, ResolverRuleAssociation,
    ResourceExistsException,
    ResourceInUseException,
    ResourceNotFoundException, Tag, TargetAddress, UnknownResourceException, ValidationException,
};

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "route53resolver";

/// `ResolverEndpointStatus` values this slice reports. The model's enum
/// (`smithy_query(query="shape",shape="ResolverEndpointStatus")`) also carries
/// `AUTO_RECOVERING` and `ACTION_NEEDED`, which describe a degraded endpoint the
/// emulator never produces.
pub const ENDPOINT_STATUS_CREATING: &str = "CREATING";
pub const ENDPOINT_STATUS_OPERATIONAL: &str = "OPERATIONAL";
pub const ENDPOINT_STATUS_UPDATING: &str = "UPDATING";
pub const ENDPOINT_STATUS_DELETING: &str = "DELETING";

/// `OutpostResolverStatus` values the Resolver-on-Outpost slice reports. The
/// model's enum (`smithy_query(query="shape",shape="OutpostResolverStatus")`,
/// re-exported as [`crate::ops::types::OUTPOST_RESOLVER_STATUS_VALUES`]) also
/// carries `ACTION_NEEDED`, `FAILED_CREATION` and `FAILED_DELETION`, which
/// describe provisioning failures the emulator never produces.
pub const OUTPOST_RESOLVER_STATUS_CREATING: &str = "CREATING";
pub const OUTPOST_RESOLVER_STATUS_OPERATIONAL: &str = "OPERATIONAL";
pub const OUTPOST_RESOLVER_STATUS_UPDATING: &str = "UPDATING";
pub const OUTPOST_RESOLVER_STATUS_DELETING: &str = "DELETING";

/// The `InstanceCount` a Resolver on an Outpost is created with when the
/// request names none, and the smallest one it may be created or updated with.
///
/// `CreateOutpostResolver`'s `InstanceCount` doc: "Number of Amazon EC2
/// instances for the Resolver on Outpost. The default and minimal value is 4."
/// The model puts no `range` on `InstanceCount`, so the minimum is enforced by
/// the handlers, which refuse a smaller count with the `ValidationException`
/// both operations model.
pub const OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT: i32 = 4;

/// `IpAddressStatus` values this slice reports, out of the model's enum
/// (`smithy_query(query="shape",shape="IpAddressStatus")`); the rest describe
/// failures the emulator never produces.
pub const IP_STATUS_CREATING: &str = "CREATING";
pub const IP_STATUS_ATTACHED: &str = "ATTACHED";
pub const IP_STATUS_UPDATING: &str = "UPDATING";

/// How many IP addresses one Resolver endpoint may hold, from the model's
/// `IpAddressesRequest` length (2..20,
/// `smithy_query(query="shape",shape="IpAddressesRequest")`): the list
/// `CreateResolverEndpoint` takes is the endpoint's address set, so the bounds
/// the model puts on that list are the bounds the set keeps afterwards. The
/// operations that add and remove one address at a time enforce them here,
/// because no modelled constraint covers an endpoint that already exists.
pub const MIN_ENDPOINT_IP_ADDRESSES: usize = 2;

/// See [`MIN_ENDPOINT_IP_ADDRESSES`].
pub const MAX_ENDPOINT_IP_ADDRESSES: usize = 20;

/// `ResolverEndpointDirection` value for an endpoint that "forwards DNS queries
/// from the DNS service for a VPC to your network", from the model's enum
/// (`smithy_query(query="shape",shape="ResolverEndpointDirection")`); it is the
/// direction a Resolver rule routes its queries through.
pub const ENDPOINT_DIRECTION_OUTBOUND: &str = "OUTBOUND";

/// `RuleTypeOption` values, the model's whole enum
/// (`smithy_query(query="shape",shape="RuleTypeOption")`).
pub const RULE_TYPE_FORWARD: &str = "FORWARD";
pub const RULE_TYPE_SYSTEM: &str = "SYSTEM";
pub const RULE_TYPE_RECURSIVE: &str = "RECURSIVE";
pub const RULE_TYPE_DELEGATE: &str = "DELEGATE";

/// The port a rule's target address forwards to when the request names none:
/// the well-known DNS port (RFC 1035 §4.2, "the Internet standard port 53").
/// Inside `TargetAddress$Port`'s modelled range (0..65535).
pub const DNS_PORT: i32 = 53;

/// The protocol an endpoint gets when the request names none.
///
/// `CreateResolverEndpoint`'s `Protocols` doc: "None, which is treated as
/// Do53." `Do53` is a member of the model's `Protocol` enum.
pub const DEFAULT_PROTOCOL: &str = "Do53";

/// How long a resource with a transition in flight (a Resolver endpoint and
/// each IP address being attached, a rule association, a firewall domain list
/// whose domains were just updated) reports its transient status before the
/// clock owes the move to the settled one.
///
/// AWS answers `CreateResolverEndpoint` with `Status: CREATING` and takes
/// minutes to reach `OPERATIONAL`; an emulator must not make a caller wait that
/// long, so the transition is owed after this span instead. It is deliberately
/// not zero: a caller that reads straight after the create still observes the
/// transient status, exactly as it would against AWS.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// Page size applied when a request omits `MaxResults`: the maximum the model's
/// `MaxResults` range (1..100) allows, so an unpaged caller sees everything it
/// can in one page.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most Resolver endpoints this store holds.
///
/// `CreateResolverEndpoint` admits any new `CreatorRequestId`, so without this
/// bound a caller looping creates would grow one emulator process without
/// limit (each record carries up to 20 IP addresses plus its tags and security
/// groups). A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_endpoint`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record, and a delete frees a slot. Bounding the endpoints bounds
/// [`ComAmazonawsRoute53resolverRoute53resolverState::creator_request_id_index`]
/// too: it only ever holds one entry per stored endpoint.
pub const MAX_STORED_ENDPOINTS: usize = 1_000;

/// The most `Filters` one `ListResolverEndpoints` request may carry, and the
/// most filter values it may carry across all of them.
///
/// The model constrains neither `Filters` nor `Filter$Values`, so without these
/// the only ceiling is the transport body cap — and the scan's cost is the
/// number of stored endpoints MULTIPLIED by the filter terms, so an uncapped
/// request would turn one call into unbounded CPU while the store lock is held.
/// The documented catalog has 7 members, so ten filters is already more than any
/// meaningful query needs. A request past either cap is refused with the
/// `InvalidParameterException` the operation models, before the store is locked.
pub const MAX_FILTERS: usize = 10;

/// See [`MAX_FILTERS`].
pub const MAX_FILTER_VALUES: usize = 100;

/// The most Resolvers on Outposts this store holds, for the same reason as
/// [`MAX_STORED_ENDPOINTS`]: `CreateOutpostResolver` admits any new
/// `CreatorRequestId`, so without a bound a caller looping creates grows one
/// emulator process without limit. A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_outpost_resolver`]
/// with the `ServiceQuotaExceededException` that operation models ("Fulfilling
/// the request would cause one or more quotas to be exceeded"); a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record, and a delete frees a slot.
pub const MAX_STORED_OUTPOST_RESOLVERS: usize = 1_000;

/// The most `SecurityGroupIds` one endpoint may be created with.
///
/// The model constrains the list's length no more than it does `Filters`, and a
/// stored endpoint's groups are walked once per filtered scan, so this bounds
/// both what one record can hold and the per-record cost of filtering on
/// `SecurityGroupIds`. A create past it is refused with the
/// `LimitExceededException` the operation models.
pub const MAX_SECURITY_GROUP_IDS: usize = 100;

/// The `MaxResults` bounds the model declares for both list operations.
const MAX_RESULTS_RANGE: (i32, i32) = (1, 100);

/// Domain-separation string mixed into a pagination cursor's checksum. NOT a
/// secret and not a security boundary: it exists so a `NextToken` this emulator
/// never minted is detected and refused, instead of being silently read as
/// "start from the beginning".
const PAGE_TOKEN_DOMAIN: &str = "route53resolver/page-token/v1";

/// The Route 53 Resolver service error code reported when a request names a
/// Resolver endpoint id the service holds no endpoint under.
///
/// Route 53 Resolver carries its own `RSLVR-nnnnn` code inside every exception
/// message, because the HTTP status (400 for every modelled Resolver error) and
/// the exception shape name together still do not say which Resolver fault
/// occurred — the code is what operators and tooling key on. The model states
/// no message text, so this code comes from the recorded responses of the
/// endpoint operations that raise `ResourceNotFoundException`.
pub const RSLVR_CODE_ENDPOINT_NOT_FOUND: &str = "RSLVR-00501";

/// The Route 53 Resolver service error code reported when a request asks a
/// Resolver endpoint to give up an IP address it is not allowed to lose,
/// because the endpoint would be left with fewer than the fewest addresses one
/// may hold.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the operation that raises
/// `InvalidRequestException` for it.
pub const RSLVR_CODE_ENDPOINT_IP_BELOW_MINIMUM: &str = "RSLVR-00505";

/// The Route 53 Resolver service error code reported when a request asks for a
/// rule of a type Resolver answers itself, yet still names where the rule should
/// forward queries to (a Resolver endpoint, target IP addresses, or both).
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes from
/// the recorded responses of the rule operation that raises
/// `InvalidRequestException` for it.
pub const RSLVR_CODE_SYSTEM_RULE_FORWARDING_TARGET: &str = "RSLVR-00707";

/// The Route 53 Resolver service error code reported when a request asks for a
/// rule that forwards queries onward, yet does not say where to: it names no
/// Resolver endpoint to forward through, or no target IP addresses to forward
/// to, or neither.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`], and it is
/// the counterpart of [`RSLVR_CODE_SYSTEM_RULE_FORWARDING_TARGET`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes from
/// the recorded responses of the rule operation that raises
/// `InvalidRequestException` for it.
pub const RSLVR_CODE_FORWARD_RULE_FORWARDING_TARGET_REQUIRED: &str = "RSLVR-00706";

/// The Route 53 Resolver service error code reported when a request asks for a
/// Resolver on an Outpost with fewer EC2 instances than `InstanceCount`'s
/// documented minimum.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred — and this operation raises `ValidationException` for several
/// distinct faults — so the code is the only discriminator. The model states no
/// message text, so this code comes from the recorded responses of the
/// Resolver-on-Outpost write operations.
pub const RSLVR_CODE_INSTANCE_COUNT_BELOW_MINIMUM: &str = "RSLVR-03002";

/// The Route 53 Resolver service error code reported when a request asks for a
/// Resolver on an Outpost that the account has not got.
///
/// Carried for the same reason as [`RSLVR_CODE_INSTANCE_COUNT_BELOW_MINIMUM`]:
/// the create operation raises `ValidationException` for several distinct
/// faults, so the code is the only discriminator between them. The model states
/// no message text, so this code comes from the recorded responses of the
/// Resolver-on-Outpost create operation.
pub const RSLVR_CODE_OUTPOST_NOT_FOUND: &str = "RSLVR-03007";

/// The Route 53 Resolver service error code reported when a request names a
/// Resolver rule the service holds no rule under.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the rule operations that resolve a
/// `ResolverRuleId` and raise `ResourceNotFoundException`.
pub const RSLVR_CODE_RULE_NOT_FOUND: &str = "RSLVR-00703";

/// The Route 53 Resolver service error code reported when a delete names a
/// Resolver rule that is still associated with a VPC.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the rule delete operation that raises
/// `ResourceInUseException` for it.
pub const RSLVR_CODE_RULE_IN_USE: &str = "RSLVR-00702";

/// The Route 53 Resolver service error code reported when a request addresses a
/// rule-to-VPC association the service holds no association for.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the operation that addresses an association by
/// its rule/VPC pair and raises `ResourceNotFoundException`.
pub const RSLVR_CODE_RULE_VPC_ASSOCIATION_NOT_FOUND: &str = "RSLVR-00800";

/// The Route 53 Resolver service error code reported when a request names a
/// resource the caller cannot address: one the service holds nothing under, and
/// one it keeps to itself.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the tag operations and of the DNS Firewall
/// domain-list operations, which report this one code for every resource a
/// caller names but cannot address. See [`resource_not_found_message`].
pub const RSLVR_CODE_RESOURCE_NOT_FOUND: &str = "RSLVR-02025";

/// `ResourceNotFoundException$ResourceType`, "the type of resource that doesn't
/// exist": the modelled shape name of a Resolver endpoint.
pub const RESOURCE_TYPE_RESOLVER_ENDPOINT: &str = "ResolverEndpoint";

/// `ResourceNotFoundException$ResourceType` for a Resolver on an Outpost: the
/// modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_OUTPOST_RESOLVER: &str = "OutpostResolver";

/// The same, for a Resolver rule and for one rule-to-VPC association.
pub const RESOURCE_TYPE_RESOLVER_RULE: &str = "ResolverRule";
pub const RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION: &str = "ResolverRuleAssociation";

/// The `ResolverRuleStatus` values a stored rule reports.
///
/// The model's enum (`smithy_query(query="shape",shape="ResolverRuleStatus")`)
/// is `COMPLETE | DELETING | UPDATING | FAILED`: it has no provisioning value,
/// so a created rule is settled the moment it exists and the clock owes it no
/// transition. `DELETING` is what the rule a delete has just taken out reports,
/// as that call's answer "including the status of the request"; the settings an
/// update writes are likewise in effect when that call answers, so `UPDATING`
/// and `FAILED` describe work this slice never leaves outstanding.
pub const RULE_STATUS_COMPLETE: &str = "COMPLETE";
pub const RULE_STATUS_DELETING: &str = "DELETING";

/// `ResolverRuleAssociationStatus` values this slice reports, out of the
/// model's enum (`smithy_query(query="shape",shape="ResolverRuleAssociationStatus")`);
/// `DELETING` is what the association a disassociate has just taken out
/// reports, while `FAILED` and `OVERRIDDEN` describe states this slice does not
/// produce.
pub const RULE_ASSOCIATION_STATUS_CREATING: &str = "CREATING";
pub const RULE_ASSOCIATION_STATUS_COMPLETE: &str = "COMPLETE";
pub const RULE_ASSOCIATION_STATUS_DELETING: &str = "DELETING";

/// `ShareStatus` for a resource this account owns and has not shared: a member
/// of the model's `ShareStatus` enum
/// (`smithy_query(query="shape",shape="ShareStatus")`). The emulator has one
/// account, so no Resolver rule and no query logging configuration it mints is
/// shared with another.
pub const SHARE_STATUS_NOT_SHARED: &str = "NOT_SHARED";

/// `SortOrder` for the descending order a list operation may be asked for, from
/// the model's enum (`smithy_query(query="shape",shape="SortOrder")`,
/// re-exported as [`crate::ops::types::SORT_ORDER_VALUES`]); the other value,
/// `ASCENDING`, is the order a sorted list reports when the request names none.
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The most Resolver rules, and rule associations, this store holds.
///
/// Both creates admit any new request (`CreateResolverRule` any unseen
/// `CreatorRequestId`, `AssociateResolverRule` any unseen rule/VPC pair), so
/// without these bounds a caller looping creates would grow one emulator
/// process without limit. A create past the bound is REFUSED with the
/// `LimitExceededException` both operations model; a retry of a stored
/// `CreatorRequestId` stays allowed, because it allocates no new record.
pub const MAX_STORED_RULES: usize = 1_000;

/// See [`MAX_STORED_RULES`].
pub const MAX_STORED_RULE_ASSOCIATIONS: usize = 1_000;

/// The `FirewallDomainListStatus` a domain list this account owns reports once
/// no import or update is under way.
///
/// The model's enum (`smithy_query(query="shape",shape="FirewallDomainListStatus")`,
/// re-exported as [`crate::ops::types::FIREWALL_DOMAIN_LIST_STATUS_VALUES`]) is
/// `COMPLETE | COMPLETE_IMPORT_FAILED | IMPORTING | DELETING | UPDATING`: the
/// other values describe an import, an update or a delete in flight, none of
/// which a create performs — `CreateFirewallDomainList` "creates an empty
/// firewall domain list", which is settled the moment it exists, so the clock
/// owes it no transition.
pub const FIREWALL_DOMAIN_LIST_STATUS_COMPLETE: &str = "COMPLETE";

/// The `FirewallDomainListStatus` a domain list reports while a change to its
/// domains is still settling, the model's `UPDATING`: it is what
/// `UpdateFirewallDomains` reports as "the status of the UpdateFirewallDomains
/// request". The move to [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`] is the one
/// the clock owes, [`PROVISIONING_DELAY`] after the call that started it.
pub const FIREWALL_DOMAIN_LIST_STATUS_UPDATING: &str = "UPDATING";

/// The `FirewallDomainListStatus` a domain list reports once a delete has been
/// accepted for it, the model's `DELETING`: `DeleteFirewallDomainList` answers
/// with "the domain list that you just deleted", which is on its way out and so
/// is no longer in the settled status it held until that call.
pub const FIREWALL_DOMAIN_LIST_STATUS_DELETING: &str = "DELETING";

/// The most domains one `UpdateFirewallDomains` request may carry.
///
/// `UpdateFirewallDomains$Domains`'s doc states it: "There is a limit of 1000
/// domains per request." A request past it is refused with the
/// `LimitExceededException` the operation models, before the store is locked,
/// so nothing is written.
pub const MAX_DOMAINS_PER_UPDATE: usize = 1_000;

/// The most domains one stored domain list holds.
///
/// Not a published AWS quota — the same reasoning as [`MAX_SECURITY_GROUP_IDS`]:
/// one call adds at most [`MAX_DOMAINS_PER_UPDATE`] domains and may be repeated
/// without limit, so without this bound a caller looping updates would grow one
/// emulator process without limit. An update that would carry a list past it is
/// refused with the `LimitExceededException` the operation models, and the
/// stored list is left exactly as it was.
pub const MAX_DOMAINS_PER_LIST: usize = 10_000;

/// `ResourceNotFoundException$ResourceType` for a firewall domain list: the
/// modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_FIREWALL_DOMAIN_LIST: &str = "FirewallDomainList";

/// The most firewall domain lists this store holds, for the same reason as
/// [`MAX_STORED_ENDPOINTS`]: `CreateFirewallDomainList` admits any new
/// `CreatorRequestId`, so without a bound a caller looping creates would grow
/// one emulator process without limit. A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_firewall_domain_list`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record.
pub const MAX_STORED_FIREWALL_DOMAIN_LISTS: usize = 1_000;

/// The `FirewallRuleGroupStatus` a rule group reports once no change to it is
/// still settling.
///
/// The model's enum (`smithy_query(query="shape",shape="FirewallRuleGroupStatus")`,
/// re-exported as [`crate::ops::types::FIREWALL_RULE_GROUP_STATUS_VALUES`]) is
/// `COMPLETE | DELETING | UPDATING`: the other two describe a delete or an
/// update in flight, neither of which a create performs —
/// `CreateFirewallRuleGroup` "creates an empty DNS Firewall rule group", which
/// is settled the moment it exists, so the clock owes it no transition.
pub const FIREWALL_RULE_GROUP_STATUS_COMPLETE: &str = "COMPLETE";

/// The `FirewallRuleGroupStatus` a rule group reports once a delete has been
/// accepted for it, the model's `DELETING`: `DeleteFirewallRuleGroup` answers
/// with the rule group it just deleted, which is on its way out and so is no
/// longer in the settled status it held until that call — the same role
/// [`FIREWALL_DOMAIN_LIST_STATUS_DELETING`] plays for a domain list.
pub const FIREWALL_RULE_GROUP_STATUS_DELETING: &str = "DELETING";

/// `ResourceNotFoundException$ResourceType` for a firewall rule group: the
/// modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_FIREWALL_RULE_GROUP: &str = "FirewallRuleGroup";

/// The most firewall rule groups this store holds, for the same reason as
/// [`MAX_STORED_FIREWALL_DOMAIN_LISTS`]: `CreateFirewallRuleGroup` admits any
/// new `CreatorRequestId`, so without a bound a caller looping creates would
/// grow one emulator process without limit. A create past this count is REFUSED
/// by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_firewall_rule_group`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record.
pub const MAX_STORED_FIREWALL_RULE_GROUPS: usize = 1_000;

/// The most VPC firewall configurations this store holds records for.
///
/// `UpdateFirewallConfig` admits any `ResourceId`, so without a bound a caller
/// looping updates over fresh VPC ids would grow one emulator process without
/// limit — the same reasoning as [`MAX_STORED_ENDPOINTS`]. An update that would
/// need a record past this count is refused; one for a VPC the store already
/// holds a record for allocates nothing and stays allowed, and so does a read of
/// any VPC, which stores nothing at all.
pub const MAX_STORED_FIREWALL_CONFIGS: usize = 1_000;

/// The `FirewallFailOpenStatus` a VPC's firewall configuration carries until an
/// update names another value.
///
/// `FirewallConfig$FirewallFailOpen`'s doc states it: "By default, fail open is
/// disabled, which means the failure mode is closed." It is a member of the
/// model's `FirewallFailOpenStatus` enum, re-exported as
/// [`crate::ops::types::FIREWALL_FAIL_OPEN_STATUS_VALUES`].
pub const FIREWALL_FAIL_OPEN_DISABLED: &str = "DISABLED";

/// The most `TargetIps` one Resolver rule may be created with.
///
/// The model's `TargetList` declares a minimum of 1 and NO maximum, so this is
/// the emulator's own bound on what one record can hold — the same reasoning as
/// [`MAX_SECURITY_GROUP_IDS`], not a published AWS quota. A create past it is
/// refused with the `LimitExceededException` the operation models.
pub const MAX_TARGET_IPS: usize = 100;

/// Render one Route 53 Resolver exception message: the service error code in
/// square brackets, a space, the sentence that states the fault, and the
/// terminating period.
///
/// `sentence` is passed WITHOUT its final period, so the shape of the message
/// is decided in exactly one place.
pub fn rslvr_message(code: &str, sentence: &str) -> String {
    format!("[{code}] {sentence}.")
}

/// The message Route 53 Resolver answers for a resource the caller named but
/// cannot address, with the id it named inside it.
///
/// ONE sentence for the whole class, because the service reports one
/// ([`RSLVR_CODE_RESOURCE_NOT_FOUND`]) for it: a resource this account never
/// created, and a resource the service publishes and keeps to itself. A caller
/// that cannot address a resource learns nothing else about it — which is what
/// makes a resource it may not write indistinguishable from one that does not
/// exist.
pub fn resource_not_found_message(resource_id: &str) -> String {
    rslvr_message(
        RSLVR_CODE_RESOURCE_NOT_FOUND,
        &format!("Can’t find the resource with ID \"{resource_id}\""),
    )
}

/// The `ResourceNotFoundException` every Resolver endpoint operation answers
/// for an id it holds no endpoint under.
///
/// Built here rather than in each handler so the four operations that resolve a
/// `ResolverEndpointId` (`GetResolverEndpoint`,
/// `ListResolverEndpointIpAddresses`, `UpdateResolverEndpoint`,
/// `DeleteResolverEndpoint`) cannot drift apart: all of them report the
/// RSLVR-coded message, with the requested id double-quoted inside it, and the
/// resource type the model asks for.
pub fn resolver_endpoint_not_found(resolver_endpoint_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_ENDPOINT_NOT_FOUND,
            &format!(r#"Resolver endpoint with ID "{resolver_endpoint_id}" does not exist"#),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_ENDPOINT.to_string()),
    }
}

/// `ResourceNotFoundException$ResourceType` and
/// `ResourceExistsException$ResourceType` for one IP address of a Resolver
/// endpoint: the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_ENDPOINT_IP_ADDRESS: &str = "IpAddressResponse";

/// The `ResourceNotFoundException` an endpoint IP-address operation answers for
/// an address the named endpoint does not hold.
///
/// Built here so `AssociateResolverEndpointIpAddress`,
/// `DisassociateResolverEndpointIpAddress` and `UpdateResolverEndpoint` cannot
/// drift apart. `address` is how the message names the address that was asked
/// for, from [`describe_requested_ip_address`].
pub fn endpoint_ip_address_not_found(
    resolver_endpoint_id: &str,
    address: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Resolver endpoint '{resolver_endpoint_id}' has no IP address {address}"
        )),
        resource_type: Some(RESOURCE_TYPE_ENDPOINT_IP_ADDRESS.to_string()),
    }
}

/// How a message names the IP address an `IpAddressUpdate` pointed at: its
/// `IpId` when the request carried one ("Only when removing an IP address from
/// a Resolver endpoint: The ID of the IP address that you want to remove"),
/// otherwise the address literal, otherwise the subnet that holds it.
pub fn describe_requested_ip_address(requested: &IpAddressUpdate) -> String {
    if let Some(ip_id) = &requested.ip_id {
        return format!("with ID '{ip_id}'");
    }
    if let Some(ip) = &requested.ip {
        return format!("'{ip}'");
    }
    if let Some(ipv6) = &requested.ipv6 {
        return format!("'{ipv6}'");
    }
    match &requested.subnet_id {
        Some(subnet_id) => format!("in subnet '{subnet_id}'"),
        None => "to remove".to_string(),
    }
}

/// The `ResourceExistsException` `AssociateResolverEndpointIpAddress` answers
/// for an address the endpoint already holds: "The resource that you tried to
/// create already exists."
pub fn endpoint_ip_address_exists(
    resolver_endpoint_id: &str,
    address: &str,
) -> ResourceExistsException {
    ResourceExistsException {
        message: Some(format!(
            "Resolver endpoint '{resolver_endpoint_id}' already has IP address {address}"
        )),
        resource_type: Some(RESOURCE_TYPE_ENDPOINT_IP_ADDRESS.to_string()),
    }
}

/// The `LimitExceededException` `AssociateResolverEndpointIpAddress` answers for
/// an endpoint that already holds [`MAX_ENDPOINT_IP_ADDRESSES`] addresses.
pub fn endpoint_ip_address_limit_exceeded() -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "A Resolver endpoint can have at most {MAX_ENDPOINT_IP_ADDRESSES} IP addresses"
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_ENDPOINT.to_string()),
    }
}

/// The `InvalidRequestException` `DisassociateResolverEndpointIpAddress`
/// answers for a removal that would leave the endpoint with fewer than
/// [`MIN_ENDPOINT_IP_ADDRESSES`] addresses.
///
/// The sentence states the minimum and nothing from the request, so neither the
/// endpoint nor the address that was asked for is echoed back.
pub fn endpoint_ip_address_below_minimum() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(rslvr_message(
            RSLVR_CODE_ENDPOINT_IP_BELOW_MINIMUM,
            &format!(
                "Can only remove IP from resolver endpoints with more than {MIN_ENDPOINT_IP_ADDRESSES} IP addresses"
            ),
        )),
    }
}

/// The `ResourceNotFoundException` every Resolver-on-Outpost operation answers
/// for an id it holds no Resolver under, built here so `GetOutpostResolver`,
/// `UpdateOutpostResolver` and `DeleteOutpostResolver` cannot drift apart.
///
/// Unlike [`resolver_endpoint_not_found`] the sentence carries no `RSLVR-nnnnn`
/// code: that code comes from the recorded responses of the endpoint
/// operations, and no recording supplies one for a missing Resolver on an
/// Outpost, so none is invented here. The wire error code clients match on is
/// the shape name, `ResourceNotFoundException`, and the modelled
/// `ResourceType` says which resource was missing.
pub fn outpost_resolver_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            r#"Resolver on Outpost with ID "{id}" does not exist."#
        )),
        resource_type: Some(RESOURCE_TYPE_OUTPOST_RESOLVER.to_string()),
    }
}

/// The `ValidationException` the Resolver-on-Outpost write operations answer for
/// an `InstanceCount` below the documented minimum.
///
/// Built here, rather than in each handler, so `CreateOutpostResolver` and
/// `UpdateOutpostResolver` report the one RSLVR-coded body for this fault: the
/// code is what tells it apart from the other `ValidationException` faults of
/// those operations. The sentence states the documented minimum and nothing from
/// the request, so the rejected count is not echoed back.
pub fn outpost_resolver_instance_count_below_minimum() -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_INSTANCE_COUNT_BELOW_MINIMUM,
            &format!(
                r#"The instance count must be equal to or greater than "{OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT}""#
            ),
        )),
    }
}

/// The `ValidationException` the Resolver-on-Outpost create answers for an
/// `OutpostArn` that names no Outpost of this account.
///
/// The sentence names the Outpost by the ARN the request supplied, verbatim, so
/// the caller is told which value could not be resolved; the code
/// ([`RSLVR_CODE_OUTPOST_NOT_FOUND`]) is what tells this fault apart from the
/// operation's other `ValidationException` faults.
pub fn outpost_not_found(outpost_arn: &str) -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_OUTPOST_NOT_FOUND,
            &format!(r#"Cannot find the Outpost with Arn "{outpost_arn}""#),
        )),
    }
}

/// Whether the account holds, in `region`, the Outpost `outpost_arn` names.
///
/// A Resolver on an Outpost runs ON an Outpost: which Outposts exist is a fact
/// of the account's own AWS Outposts inventory — the one `ListOutposts` answers
/// from — and not a value this service's caller supplies, so this is the one
/// place that resolution happens. The Outposts records live in that service's
/// own store and are resolved through its own accessors, so one Outpost is
/// never described by two services differently.
///
/// An Outpost is identified within the Region it is racked in, so only an ARN
/// naming an Outpost OF `region` (and of this endpoint's partition and account)
/// resolves: an ARN of another Region names nothing this endpoint holds, which
/// is exactly how that service answers a call naming it.
///
/// LOCKING. The Outposts store lock is taken for the single lookup that answers
/// this and is released before returning; handlers call this BEFORE taking this
/// service's own lock, so the two stores are never held at once. AWS Outposts
/// never reaches this service, so no cycle exists.
pub fn account_holds_outpost<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    outpost_arn: &str,
) -> bool {
    let Some(outpost_id) = outposts::resolve_outpost_id(outpost_arn, region)
        .as_ref()
        .and_then(outposts::ResolvedIdentifier::here)
        .map(str::to_string)
    else {
        return false;
    };
    let Some(state) = outposts::service_state(engine) else {
        // No Outposts store on this engine is no Outpost inventory to read, so
        // the account holds no Outpost to put a Resolver on.
        return false;
    };
    let store = state.lock();
    store.outpost(region, &outpost_id).is_some()
}

/// The `InvalidRequestException` the rule write path answers for a rule that
/// forwards queries onward but does not say where to.
///
/// One body for the whole precondition — a missing `ResolverEndpointId`, missing
/// `TargetIps`, or both — because a forwarding rule needs the endpoint and the
/// target addresses TOGETHER: either one alone describes no forwarding path, so
/// it is one fault and carries one service error code.
pub fn forward_rule_requires_forwarding_target() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(rslvr_message(
            RSLVR_CODE_FORWARD_RULE_FORWARDING_TARGET_REQUIRED,
            &format!(
                "Resolver endpoint ID and target IP are required for {RULE_TYPE_FORWARD} type resolver rule"
            ),
        )),
    }
}

/// The `InvalidRequestException` the rule write path answers for a rule of a
/// type Resolver answers itself that still names where to forward queries to —
/// the mirror image of [`forward_rule_requires_forwarding_target`], and one body
/// for the endpoint, the target addresses or both.
pub fn system_rule_forbids_forwarding_target() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(rslvr_message(
            RSLVR_CODE_SYSTEM_RULE_FORWARDING_TARGET,
            &format!(
                "Cannot specify resolver endpoint ID and target IP for {RULE_TYPE_SYSTEM} type resolver rule"
            ),
        )),
    }
}

/// The `InvalidRequestException` a rule write answers when the rule's TYPE
/// forbids the forwarding members the request names, or `None` when that type
/// admits them.
///
/// `TargetIps` "is available only when the value of Rule type is FORWARD", and
/// `ResolverEndpointId` is the endpoint the queries named by those addresses
/// pass through, so a rule of any other type carries neither. Decided here
/// rather than in each handler so the create and the update of one rule cannot
/// disagree about which member its type admits; the type is the one the rule
/// HAS (the request's on a create, the stored record's on an update).
pub fn forwarding_target_forbidden(
    rule_type: &str,
    names_target_ips: bool,
    names_resolver_endpoint: bool,
) -> Option<InvalidRequestException> {
    // A rule of a type Resolver answers itself names no forwarding target at
    // all, so the endpoint is refused on its own as well as with the addresses.
    if rule_type == RULE_TYPE_SYSTEM && (names_target_ips || names_resolver_endpoint) {
        return Some(system_rule_forbids_forwarding_target());
    }
    // "when creating a DELEGATE rule, you must not provide the TargetIps
    // parameter. If you provide the TargetIps, you may receive an ERROR message
    // similar to 'Delegate resolver rules need to specify a nameserver name'" —
    // the model states that wording, so it is reported.
    if rule_type == RULE_TYPE_DELEGATE && names_target_ips {
        return Some(InvalidRequestException {
            message: Some("Delegate resolver rules need to specify a nameserver name.".to_string()),
        });
    }
    if rule_type != RULE_TYPE_FORWARD && names_target_ips {
        return Some(InvalidRequestException {
            message: Some(format!(
                "TargetIps is available only for a rule with RuleType {RULE_TYPE_FORWARD}, not {rule_type}"
            )),
        });
    }
    None
}

/// What is wrong with the target-address list a rule write carries, when
/// anything is.
///
/// The two faults are reported with DIFFERENT modelled errors, so
/// [`target_address_fault`] names which one it found and the handler lifts it
/// into the error its own operation models.
#[derive(Debug)]
pub enum TargetAddressFault {
    /// More target addresses than one stored rule holds ([`MAX_TARGET_IPS`]).
    TooMany(LimitExceededException),
    /// An address that names no address at all, or a list mixing the two
    /// address families.
    Invalid(InvalidParameterException),
}

/// Whether the target addresses a rule write carries can be stored as they are,
/// and what is wrong with them when they cannot.
///
/// One function for every write of a rule's `TargetIps`, so the bound and the
/// per-address preconditions a create enforces are the ones an update enforces
/// too.
pub fn target_address_fault(targets: &[TargetAddress]) -> Option<TargetAddressFault> {
    // The model puts no maximum on `TargetList`, so the list one record can
    // hold is bounded here.
    if targets.len() > MAX_TARGET_IPS {
        return Some(TargetAddressFault::TooMany(LimitExceededException {
            message: Some(format!(
                "A Resolver rule accepts at most {MAX_TARGET_IPS} target IP addresses in this emulator"
            )),
            resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
        }));
    }

    // Each target address names where to forward to, so one that carries
    // neither `Ip` nor `Ipv6` says nothing; and `TargetIps`: "You can specify
    // either Ipv4 or Ipv6 addresses but not both in the same rule."
    let ipv4_targets = targets.iter().filter(|target| target.ip.is_some()).count();
    let ipv6_targets = targets.iter().filter(|target| target.ipv6.is_some()).count();
    if ipv4_targets + ipv6_targets < targets.len() {
        return Some(TargetAddressFault::Invalid(InvalidParameterException {
            message: "Each target address must specify Ip or Ipv6".to_string(),
            field_name: Some("TargetIps".to_string()),
        }));
    }
    if ipv4_targets > 0 && ipv6_targets > 0 {
        return Some(TargetAddressFault::Invalid(InvalidParameterException {
            message: "A Resolver rule can specify either Ip or Ipv6 target addresses, not both"
                .to_string(),
            field_name: Some("TargetIps".to_string()),
        }));
    }

    None
}

/// The target addresses a rule stores for the list a request carried: the same
/// addresses, in the same order, with the members the request left out filled
/// in from the documented defaults.
///
/// The order is the request's: `TargetIps` is the list of resolvers a query is
/// forwarded to, so the list a read reports is the list the write was given,
/// neither sorted nor de-duplicated.
pub fn normalize_target_addresses(targets: &[TargetAddress]) -> Vec<TargetAddress> {
    targets
        .iter()
        .map(|target| TargetAddress {
            ip: target.ip.clone(),
            // DNS answers on port 53 (RFC 1035 §4.2), which is the port a
            // target address that names none forwards to.
            port: Some(target.port.unwrap_or(DNS_PORT)),
            ipv6: target.ipv6.clone(),
            // The `Protocol` enum's `Do53` is what "None" means for a Resolver
            // protocol, per `CreateResolverEndpoint`'s `Protocols` doc: "None,
            // which is treated as Do53."
            protocol: Some(
                target
                    .protocol
                    .clone()
                    .unwrap_or_else(|| DEFAULT_PROTOCOL.to_string()),
            ),
            // "only used if the Protocol of the TargetAddress is DoH", so it is
            // echoed, never invented.
            server_name_indication: target.server_name_indication.clone(),
        })
        .collect()
}

/// The `ResourceNotFoundException` the rule operations answer for an id they
/// hold no Resolver rule under.
///
/// Built here rather than in each handler so every operation that resolves a
/// `ResolverRuleId` (`GetResolverRule`, `UpdateResolverRule`,
/// `DeleteResolverRule`, `AssociateResolverRule`) reports the one body: the
/// RSLVR-coded message, with the requested id double-quoted inside it, and the
/// resource type the model asks for.
pub fn resolver_rule_not_found(resolver_rule_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_NOT_FOUND,
            &format!(r#"Resolver rule with ID "{resolver_rule_id}" does not exist"#),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
    }
}

/// The `ResourceNotFoundException` `GetResolverRuleAssociation` answers for an
/// id it holds no rule association under. See
/// [`outpost_resolver_not_found`] on the absent service error code: the
/// recordings available here state no code for an association addressed by its
/// OWN id, so none is invented for it.
pub fn resolver_rule_association_not_found(
    resolver_rule_association_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            r#"Resolver rule association with ID "{resolver_rule_association_id}" does not exist."#
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION.to_string()),
    }
}

/// The `ResourceNotFoundException` `DisassociateResolverRule` answers when the
/// store holds no association of that RULE with that VPC — the pair that call
/// addresses an association by, rather than the association's own id.
///
/// The missing resource the sentence names is the ASSOCIATION itself, not a
/// property of the rule, and its two variable parts are the rule and the VPC the
/// request named.
pub fn resolver_rule_vpc_association_not_found(
    resolver_rule_id: &str,
    vpc_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_VPC_ASSOCIATION_NOT_FOUND,
            &format!(
                r#"Resolver Rule Association between Resolver Rule "{resolver_rule_id}" and VPC "{vpc_id}" does not exist"#
            ),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION.to_string()),
    }
}

/// The `ResourceInUseException` `DeleteResolverRule` answers for a rule that is
/// still associated with a VPC.
///
/// The sentence is entirely fixed: it states the remedy the caller has to apply
/// before the delete can succeed, and echoes nothing from the request or the
/// store — neither the rule, nor a VPC, nor how many associations are still
/// held.
pub fn resolver_rule_in_use() -> ResourceInUseException {
    ResourceInUseException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_IN_USE,
            "Please disassociate this resolver rule from VPC first before deleting",
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
    }
}

/// The `UnknownResourceException` the Resolver rule policy operations answer
/// for an `Arn` the store holds no Resolver rule under.
///
/// Both operations address the rule by its ARN rather than by its id, so the
/// sentence names what the request asked for: the ARN it was handed. No
/// recorded response available here states a service error code for this fault
/// — as for [`resolver_rule_association_not_found`] — so none is invented, and
/// the HTTP status (400) and the error shape are what the contract fixes.
pub fn unknown_resolver_rule_arn(arn: &str) -> UnknownResourceException {
    UnknownResourceException {
        message: Some(format!(r#"Resolver rule with ARN "{arn}" does not exist."#)),
    }
}

/// The `InvalidPolicyDocument` a resource-policy write answers for a document
/// outside the policy grammar, with `fault` stating which part of the grammar it
/// misses (from [`policy_document_fault`]).
///
/// The opening sentence is the fault the model states for the shape ("The
/// specified Resolver rule policy is invalid"); what follows names the element
/// the document is missing, so the caller can fix the document from the message
/// alone.
pub fn invalid_policy_document(fault: &str) -> InvalidPolicyDocument {
    InvalidPolicyDocument {
        message: Some(format!(
            "The specified Resolver rule policy is invalid: {fault}."
        )),
    }
}

/// The policy document a resource with no stored resource policy reports: the
/// empty JSON object, a document that states no statement and so shares the
/// resource with nobody. Inside `ResolverRulePolicy`'s modelled length
/// (0..30000, `smithy_query(query="shape",shape="ResolverRulePolicy")`).
pub const EMPTY_POLICY_DOCUMENT: &str = "{}";

/// Why `policy` is not a policy document, or `None` when it is one.
///
/// A JSON policy document is an object, and each statement of its `Statement`
/// element — one statement or an array of them — is an object that states an
/// `Effect` and an `Action` (the elements the IAM JSON policy grammar requires
/// of every statement). A document that states no statement at all is still a
/// document: it grants nothing, which is what [`EMPTY_POLICY_DOCUMENT`] is.
///
/// The modelled `ResolverRulePolicy` length (0..30000) admits the empty string
/// and any other text, so the grammar is what a policy write checks here,
/// before it touches the store: a document outside it is refused with
/// `InvalidPolicyDocument` and the resource keeps the policy it had.
pub fn policy_document_fault(policy: &str) -> Option<String> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(policy) else {
        return Some("a policy document is JSON".to_string());
    };
    if !document.is_object() {
        return Some("a policy document is a JSON object".to_string());
    }
    for (index, statement) in policy_statements(&document).iter().enumerate() {
        let Some(members) = statement.as_object() else {
            return Some(format!("statement {index} is not a JSON object"));
        };
        for element in ["Effect", "Action"] {
            if !members.contains_key(element) {
                return Some(format!("statement {index} states no {element} element"));
            }
        }
    }
    None
}

/// The statements of a policy document: the elements of its `Statement` array,
/// or the single statement when the element is one statement rather than an
/// array, or none when the document states no `Statement` at all.
fn policy_statements(document: &serde_json::Value) -> Vec<&serde_json::Value> {
    match document.get("Statement") {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    }
}

/// Every principal one statement's `Principal` element names, in the forms the
/// policy grammar writes them: the element is either one string (`*`) or a map
/// from principal type to one value or a list of values.
fn policy_principals(principal: &serde_json::Value) -> Vec<&str> {
    match principal {
        serde_json::Value::String(value) => vec![value.as_str()],
        serde_json::Value::Object(by_type) => by_type
            .values()
            .flat_map(|value| match value {
                serde_json::Value::Array(items) => {
                    items.iter().filter_map(serde_json::Value::as_str).collect()
                }
                single => single.as_str().into_iter().collect::<Vec<&str>>(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The account one principal value names: the account field of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or the value
/// itself when it is a bare account id.
///
/// `None` for anything that names no single account — `*`, or a principal of
/// another kind such as a service — and none of those is the owning account.
fn principal_account(value: &str) -> Option<&str> {
    if let Some(rest) = value.strip_prefix("arn:") {
        return rest.split(':').nth(3).filter(|account| !account.is_empty());
    }
    if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
        return Some(value);
    }
    None
}

/// Whether `policy` grants anything to a principal other than `owner_account`,
/// the account that owns the resource the policy would be attached to.
///
/// A resource policy on a Resolver resource is how it is shared with ANOTHER
/// account, so a document whose principals are all the owning account — or that
/// names no principal at all — shares the resource with nobody and leaves it
/// with no policy. Read with [`policy_principals`] and [`principal_account`], so
/// an account named by its id and the same account named by an ARN carrying it
/// count as the one account.
pub fn policy_shares_with_other_account(policy: &str, owner_account: &str) -> bool {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(policy) else {
        return false;
    };
    policy_statements(&document).iter().any(|statement| {
        statement
            .get("Principal")
            .map(policy_principals)
            .unwrap_or_default()
            .iter()
            .any(|value| principal_account(value) != Some(owner_account))
    })
}

/// Canonicalise the domain name a Resolver rule selects on to a fully-qualified
/// DNS name: the name followed by exactly one root label, written as a trailing
/// dot (RFC 1035 §3.1 — the final zero-length label of a name relative to the
/// root).
///
/// Resolver reports a rule's `DomainName` in that canonical form whatever form
/// the request wrote it in, so `example.com` and `example.com.` name one domain
/// and a read of either rule reports the same member. The modelled `DomainName`
/// length (1..256) leaves room for the root label on a name at the 255-octet DNS
/// limit.
pub fn canonical_domain_name(domain_name: &str) -> String {
    let relative = domain_name.trim_end_matches('.');
    if relative.is_empty() {
        // Nothing but root labels names the root itself.
        ".".to_string()
    } else {
        format!("{relative}.")
    }
}

/// Whether `name` satisfies the model's `Name` pattern,
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`
/// (`smithy_query(query="shape",shape="Name")`): at least one character, every
/// character a letter, a digit, a hyphen, an underscore, an apostrophe or a
/// space, and not digits alone.
///
/// The pattern's negative lookahead is outside the syntax the boundary
/// validator's regex engine accepts, so a name the pattern excludes reaches a
/// handler; the operations that accept a `Name` refuse it here instead, with the
/// client error each of them models.
pub fn name_satisfies_pattern(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '\'' | ' '))
        && !name.chars().all(|c| c.is_ascii_digit())
}

/// The expression a refusal QUOTES for `Name`: the model's pattern
/// (`smithy_query(query="shape",shape="Name")` reports
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`) WITHOUT the `^`/`$` pair the model
/// wraps it in.
///
/// The pair is dropped because the recorded refusal of a digits-only `Name`
/// quotes the expression inside it: those anchors are how a model turns a find
/// into a whole-value match, and what the service publishes is the expression a
/// value has to satisfy — so a client that lifts it out of the message and
/// re-validates locally gets the one the service advertises. The CHECK is made
/// against the whole pattern, by [`name_satisfies_pattern`]; this constant is
/// only what the message states.
pub const NAME_PATTERN_AS_STATED: &str = r"(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)";

/// The message a `Name` the model's pattern excludes is refused with.
///
/// A name the pattern excludes violates a MODELLED constraint, so the sentence
/// is the framework's own for that class rather than one worded by hand: the
/// violation count, the rejected value, the member path in the lowerCamel
/// spelling every path of this emulator uses, and the expression the value had
/// to satisfy. Only that expression is this service's own
/// ([`NAME_PATTERN_AS_STATED`]), so a name refused here differs from one the
/// boundary validator refuses in the clause's requirement half alone.
pub fn name_pattern_violation(name: &str) -> String {
    let violation = ::aws_smithy_schema_server::constraints::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(NAME_MEMBER),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(
            NAME_PATTERN_AS_STATED.to_string(),
        ),
        // `Name`'s modelled length (0..64) is checked before a handler runs, so
        // the value echoed here is already bounded.
        value: Some(name.to_string()),
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
        .unwrap_or_else(|| String::from("Name is outside the pattern the model declares for it"))
}

/// The modelled member name the `Name` refusal states, as the model spells it.
const NAME_MEMBER: &str = "Name";

/// The `ResourceNotFoundException` the firewall domain-list operations answer
/// for a list the caller cannot address, built here so every operation that
/// resolves a `FirewallDomainListId` reports the same body.
///
/// It is the answer both for an id this account created no list under and for
/// one that names a list DNS Firewall publishes itself on a path that would
/// WRITE it ([`managed_firewall_domain_list`]): a published list is the
/// service's own, so nothing this account does changes it and it is not visible
/// as a writable resource at all. Reporting one body for both is what makes
/// those two indistinguishable, as [`resource_not_found_message`] describes.
pub fn firewall_domain_list_not_found(
    firewall_domain_list_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(firewall_domain_list_id)),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_DOMAIN_LIST.to_string()),
    }
}

/// The `ConflictException` a DNS Firewall delete answers for a resource whose
/// last change has not settled yet, built here so the deletes that refuse one
/// report the same body.
///
/// The sentence is the one the model states for this error class
/// (`smithy_query(query="shape",shape="ConflictException")`): "The requested
/// state transition isn't valid. For example, you can't delete a firewall
/// domain list if it is in the process of being deleted". It names no resource
/// and no status, so nothing is interpolated into it.
pub fn state_transition_conflict() -> ConflictException {
    ConflictException {
        message: Some("The requested state transition isn't valid.".to_string()),
    }
}

/// The `ResourceNotFoundException` the firewall rule-group operations answer for
/// a rule group the caller cannot address, built here so every operation that
/// resolves a `FirewallRuleGroupId` reports the same body.
///
/// It carries the same RSLVR-coded sentence as every other resource of this
/// service a caller names but cannot address
/// ([`resource_not_found_message`]), with the rule group's modelled shape name
/// as the resource type.
pub fn firewall_rule_group_not_found(firewall_rule_group_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(firewall_rule_group_id)),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_RULE_GROUP.to_string()),
    }
}

/// The key a firewall domain list decides membership on: the domain name, case
/// folded.
///
/// DNS names are case-insensitive (RFC 4343 §1), so `Example.com` and
/// `example.com` name ONE domain: adding the second to a list that already
/// holds the first adds nothing, and removing either removes the entry. Only
/// the comparison is folded — a stored domain keeps the text the request wrote.
pub fn firewall_domain_key(domain: &str) -> String {
    domain.to_ascii_lowercase()
}

/// Whether `domain` satisfies the requirements
/// `UpdateFirewallDomains$Domains`'s doc states for "each domain specification
/// in your domain list": "It can optionally start with * (asterisk). With the
/// exception of the optional starting asterisk, it must only contain the
/// following characters: A-Z, a-z, 0-9, - (hyphen)."
///
/// The doc's character list omits the two separators a domain name is written
/// with, which its own wildcard form (`*.example.com`) requires, so a name is
/// read as DNS labels: the optional leading asterisk, then labels of the
/// documented characters (plus the underscore of an underscored service name,
/// RFC 8552 §1) separated by dots, each label non-empty and within the 63
/// octets RFC 1035 §2.3.4 allows, optionally ending in the root label written
/// as a trailing dot (RFC 1035 §3.1). The length is the model's own
/// (`FirewallDomainName`, 1..255), already enforced before a handler runs.
pub fn firewall_domain_is_wellformed(domain: &str) -> bool {
    let name = match domain.strip_prefix('*') {
        Some(rest) => rest.strip_prefix('.').unwrap_or(rest),
        None => domain,
    };
    let labels = name.strip_suffix('.').unwrap_or(name);
    !labels.is_empty()
        && labels.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
}

/// The `ManagedOwnerName` of every domain list DNS Firewall itself publishes.
///
/// `FirewallDomainList$ManagedOwnerName`'s doc states it: "the managed domain
/// list `AWSManagedDomainsMalwareDomainList` has the managed owner name
/// `Route 53 Resolver DNS Firewall`". It fits `ServicePrinciple` (length
/// 1..512).
pub const MANAGED_FIREWALL_DOMAIN_LIST_OWNER: &str = "Route 53 Resolver DNS Firewall";

/// `DomainListType` values, the model's whole enum
/// (`smithy_query(query="shape",shape="DomainListType")`).
pub const DOMAIN_LIST_TYPE_THREAT: &str = "THREAT";
pub const DOMAIN_LIST_TYPE_CONTENT: &str = "CONTENT";

/// One domain list DNS Firewall publishes itself, rather than one this account
/// created.
///
/// A managed list belongs to the service, not to the account that reads it: it
/// is visible to every account in every region, no request created it, and
/// nothing an account does changes it. So it is NOT a stored record — it is a
/// published catalog entry, and every member a response reports for it is
/// computed from this entry plus the region the request was signed for.
#[derive(Debug, Clone, Copy)]
pub struct ManagedFirewallDomainList {
    /// The list's `Name`, which is also how AWS publishes it.
    pub name: &'static str,
    /// The `ManagedListType` this list reports: "the type of the managed domain
    /// list, for example `THREAT`".
    pub managed_list_type: &'static str,
}

/// The domain lists DNS Firewall publishes to every account, in the order
/// `ListFirewallDomainLists` reports them.
///
/// The names are the ones AWS publishes for DNS Firewall's managed domain
/// lists, the first of which the model itself names in
/// `FirewallDomainList$ManagedOwnerName`'s doc. All three are threat lists, so
/// each reports [`DOMAIN_LIST_TYPE_THREAT`]; `CONTENT`
/// ([`DOMAIN_LIST_TYPE_CONTENT`]) is the other value the model's enum carries,
/// which no published list of this service uses. The GuardDuty threat list is
/// deliberately absent: it appears only for an account with GuardDuty enabled,
/// and this emulator holds no GuardDuty state to decide that from.
pub const MANAGED_FIREWALL_DOMAIN_LISTS: &[ManagedFirewallDomainList] = &[
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsMalwareDomainList",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
    },
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsBotnetCommandandControl",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
    },
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsAggregateThreatList",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
    },
];

impl ManagedFirewallDomainList {
    /// This list's `Id`, derived from its name so that every read of it — in any
    /// process, in any region — reports the same id, exactly as a published
    /// resource must. The form is [`mint_firewall_domain_list_id`]'s
    /// (`rslvr-fdl-<hex>`), so an id is recognisable as a domain list's and fits
    /// `ResourceId` (length 1..64).
    pub fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"managed-firewall-domain-list/v1");
        feed_field(&mut hasher, self.name.as_bytes());
        format!("rslvr-fdl-{}", hex::encode(&hasher.finalize()[..8]))
    }

    /// This list's `Arn`, rooted in the region the request was signed for, as
    /// every ARN this service reports is.
    pub fn arn(&self, region: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("firewall-domain-list/{}", self.id()),
        )
    }

    /// Project the catalog entry onto the modelled `FirewallDomainListMetadata`
    /// `ListFirewallDomainLists` reports.
    pub fn to_metadata(&self, region: &str) -> FirewallDomainListMetadata {
        FirewallDomainListMetadata {
            id: Some(self.id()),
            arn: Some(self.arn(region)),
            name: Some(self.name.to_string()),
            // `CreatorRequestId` is "a unique string defined by YOU to identify
            // the request": no request of this account created a published list,
            // so there is none to report. `Category` is "the category of the
            // domain list", for which neither the model nor the published list
            // states a value, so no value is invented for it.
            creator_request_id: None,
            managed_owner_name: Some(MANAGED_FIREWALL_DOMAIN_LIST_OWNER.to_string()),
            managed_list_type: Some(self.managed_list_type.to_string()),
            category: None,
        }
    }

    /// Project the catalog entry onto the modelled `FirewallDomainList`
    /// `GetFirewallDomainList` reports, so a read of an id
    /// `ListFirewallDomainLists` reported resolves to the same list.
    pub fn to_domain_list(&self, region: &str) -> FirewallDomainList {
        FirewallDomainList {
            id: Some(self.id()),
            arn: Some(self.arn(region)),
            name: Some(self.name.to_string()),
            // `DomainCount` is "the number of domain names that are specified
            // in the domain list": the domains of a published list are the
            // service's own and are held nowhere here
            // ([`managed_firewall_domain_list_access_denied`] is what a request
            // for them is answered with), so the count this reports is the
            // count this store holds for the list — none — exactly as a stored
            // record's count is derived from its own domains
            // ([`FirewallDomainListRecord::domain_count`]) rather than claimed.
            // `CreationTime`/`ModificationTime` are unknown — no call of this
            // account created or last changed the list — so neither is
            // invented.
            domain_count: Some(0),
            status: Some(FIREWALL_DOMAIN_LIST_STATUS_COMPLETE.to_string()),
            status_message: None,
            managed_owner_name: Some(MANAGED_FIREWALL_DOMAIN_LIST_OWNER.to_string()),
            creator_request_id: None,
            creation_time: None,
            modification_time: None,
            category: None,
            managed_list_type: Some(self.managed_list_type.to_string()),
        }
    }
}

/// The published domain list this `FirewallDomainListId` names, or `None` for an
/// id that names none.
///
/// Every operation that resolves a `FirewallDomainListId` consults this after
/// the store, so one id means one list throughout the service rather than
/// resolving in the operation that lists them and nowhere else.
pub fn managed_firewall_domain_list(id: &str) -> Option<&'static ManagedFirewallDomainList> {
    MANAGED_FIREWALL_DOMAIN_LISTS
        .iter()
        .find(|managed| managed.id() == id)
}

/// The `AccessDeniedException` `ListFirewallDomains` answers for a list DNS
/// Firewall publishes itself.
///
/// The domains of a published list are the service's own threat intelligence,
/// not this account's data: the list is readable as a resource
/// ([`ManagedFirewallDomainList::to_domain_list`]) while the domains inside it
/// are not readable at all, which the model's `AccessDeniedException` ("the
/// current account doesn't have the IAM permissions required to perform the
/// specified Resolver operation") is the modelled answer for. The sentence
/// states the fault and names the kind of list it is about — the owner every
/// published list reports ([`MANAGED_FIREWALL_DOMAIN_LIST_OWNER`]) — and
/// carries nothing from the request.
pub fn managed_firewall_domain_list_access_denied() -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "You don't have a permission to list the domains inside a \
             {MANAGED_FIREWALL_DOMAIN_LIST_OWNER} managed list."
        )),
    }
}

/// A firewall domain list, as stored.
///
/// `DomainCount` is DERIVED from [`Self::domains`] and never held separately, so
/// the count and the domains cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct FirewallDomainListRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub creator_request_id: String,
    /// The domain names this list holds, in the order they were added. A list
    /// is "created empty" and is populated by `ImportFirewallDomains` or
    /// `UpdateFirewallDomains`.
    pub domains: Vec<String>,
    /// The `FirewallDomainListStatus` this list reports; see
    /// [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`] on why a created list is
    /// already settled.
    pub status: String,
    pub status_message: Option<String>,
    /// `CreateFirewallDomainList$Tags`, kept with the record so they are not
    /// silently discarded. `FirewallDomainList` models no `Tags` member, so no
    /// response reports them: `ListTagsForResource` is a separate operation.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// `COMPLETE` is owed, or `None` for a list with no transition in flight
    /// (one nobody has changed since it was created empty).
    pub settled_at: Option<Instant>,
}

impl FirewallDomainListRecord {
    /// `DomainCount`: "The number of domain names that are specified in the
    /// domain list."
    pub fn domain_count(&self) -> i32 {
        i32::try_from(self.domains.len()).unwrap_or(i32::MAX)
    }

    /// This list's `FirewallDomainListStatus` as of `now`: the transient status
    /// until the transition the clock owes falls due,
    /// [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`] after it — the same shape as
    /// [`ResolverEndpointRecord::status_at`].
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            _ => FIREWALL_DOMAIN_LIST_STATUS_COMPLETE,
        }
    }

    /// Project the record onto the modelled `FirewallDomainList`.
    pub fn to_domain_list(&self, now: Instant) -> FirewallDomainList {
        FirewallDomainList {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            domain_count: Some(self.domain_count()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            // `ManagedOwnerName`, `Category` and `ManagedListType` describe "the
            // managed domain list" kinds AWS itself publishes; a list this
            // account created is not one, so all three stay unset.
            managed_owner_name: None,
            creator_request_id: Some(self.creator_request_id.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            category: None,
            managed_list_type: None,
        }
    }

    /// Project the record onto the modelled `FirewallDomainListMetadata`, the
    /// "minimal high-level information for a firewall domain list"
    /// `ListFirewallDomainLists` reports.
    ///
    /// It is the same record [`Self::to_domain_list`] projects, narrowed to the
    /// members `FirewallDomainListMetadata` declares — the metadata shape
    /// carries no `DomainCount`, `Status`, `StatusMessage` or timestamps, which
    /// is why the doc says to "call GetFirewallDomainList and
    /// ListFirewallDomains" for the full information.
    pub fn to_metadata(&self) -> FirewallDomainListMetadata {
        FirewallDomainListMetadata {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            // `ManagedOwnerName` is "used only for lists that are NOT managed by
            // you", and `Category`/`ManagedListType` describe such a list, so
            // all three stay unset for a list this account created — exactly as
            // in [`Self::to_domain_list`].
            managed_owner_name: None,
            managed_list_type: None,
            category: None,
        }
    }
}

/// A DNS Firewall rule group, as stored.
///
/// `RuleCount` is DERIVED from [`Self::rules`] and never held separately — the
/// same way [`FirewallDomainListRecord::domain_count`] derives from its domains
/// — so the count and the rules cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct FirewallRuleGroupRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    /// "The Amazon Web Services account ID for the account that created the rule
    /// group."
    pub owner_id: String,
    pub creator_request_id: String,
    /// The `ShareStatus` this rule group reports; the emulator has one account,
    /// so it is [`SHARE_STATUS_NOT_SHARED`] until a resource policy shares it.
    pub share_status: String,
    /// The rules this group filters DNS traffic with, in the order they were
    /// added — a group is created empty ("you can add rules to the new rule
    /// group by calling CreateFirewallRule"), so the rule operations are what
    /// populate it.
    pub rules: Vec<FirewallRule>,
    /// The `FirewallRuleGroupStatus` this group reports, as of
    /// [`Self::settled_at`].
    pub status: String,
    pub status_message: Option<String>,
    /// `CreateFirewallRuleGroup$Tags`, kept with the record so they are not
    /// silently discarded. `FirewallRuleGroup` models no `Tags` member, so no
    /// response reports them: `ListTagsForResource` is a separate operation.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// [`FIREWALL_RULE_GROUP_STATUS_COMPLETE`] is owed, or `None` for a group
    /// with no transition in flight (one nobody has changed since it was
    /// created empty).
    pub settled_at: Option<Instant>,
}

impl FirewallRuleGroupRecord {
    /// `RuleCount`: "The number of rules in the rule group."
    pub fn rule_count(&self) -> i32 {
        i32::try_from(self.rules.len()).unwrap_or(i32::MAX)
    }

    /// This group's `FirewallRuleGroupStatus` as of `now`: the transient status
    /// until the transition the clock owes falls due,
    /// [`FIREWALL_RULE_GROUP_STATUS_COMPLETE`] after it — the same shape as
    /// [`FirewallDomainListRecord::status_at`].
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            Some(_) => FIREWALL_RULE_GROUP_STATUS_COMPLETE,
            None => &self.status,
        }
    }

    /// Project the record onto the modelled `FirewallRuleGroup`.
    pub fn to_rule_group(&self, now: Instant) -> FirewallRuleGroup {
        FirewallRuleGroup {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            rule_count: Some(self.rule_count()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            owner_id: Some(self.owner_id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            share_status: Some(self.share_status.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
        }
    }

    /// Project the record onto the modelled `FirewallRuleGroupMetadata`, the
    /// "minimal high-level information for a firewall rule group"
    /// `ListFirewallRuleGroups` reports.
    ///
    /// It is the same record [`Self::to_rule_group`] projects, narrowed to the
    /// members `FirewallRuleGroupMetadata` declares
    /// (`smithy_query(query="shape",shape="FirewallRuleGroupMetadata")`): the
    /// metadata shape carries no `RuleCount`, `Status`, `StatusMessage` or
    /// timestamps, which is why its doc says to "call GetFirewallRuleGroup and
    /// ListFirewallRules" for the full information.
    pub fn to_metadata(&self) -> FirewallRuleGroupMetadata {
        FirewallRuleGroupMetadata {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            owner_id: Some(self.owner_id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            share_status: Some(self.share_status.clone()),
        }
    }
}

/// The DNS Firewall configuration of ONE VPC, as stored.
///
/// A VPC's firewall configuration is a singleton the service maintains itself —
/// there is no operation that creates or deletes one — so a record exists only
/// for a VPC whose configuration was moved off its default; every other VPC
/// reports the default one [`ComAmazonawsRoute53resolverRoute53resolverState::firewall_config`]
/// derives. `Id` and `OwnerId` are therefore the same whether the record exists
/// or not, so a VPC's configuration does not change identity when it is updated.
#[derive(Debug, Clone)]
pub struct FirewallConfigRecord {
    /// Creation order, for the list operation's pages (the same role as
    /// [`FirewallDomainListRecord::seq`]).
    ///
    /// `0` on the default configuration
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::firewall_config`]
    /// derives for a VPC the store holds no record for: the id sequence starts
    /// at 1, so a stored record never carries it.
    pub seq: u64,
    /// The `Id` of this configuration, [`firewall_config_id`] of its VPC.
    pub id: String,
    /// The `ResourceId`: "The ID of the VPC that this firewall configuration
    /// applies to."
    pub resource_id: String,
    /// "The Amazon Web Services account ID of the owner of the VPC."
    pub owner_id: String,
    /// One of [`crate::ops::types::FIREWALL_FAIL_OPEN_STATUS_VALUES`].
    pub firewall_fail_open: String,
}

impl FirewallConfigRecord {
    /// The record as the modelled `FirewallConfig` shape the firewall
    /// configuration operations report.
    pub fn to_firewall_config(&self) -> FirewallConfig {
        FirewallConfig {
            id: Some(self.id.clone()),
            resource_id: Some(self.resource_id.clone()),
            owner_id: Some(self.owner_id.clone()),
            firewall_fail_open: Some(self.firewall_fail_open.clone()),
        }
    }
}

/// One IP address of a Resolver endpoint, as stored.
#[derive(Debug, Clone)]
pub struct IpAddressRecord {
    /// The emulator-minted `IpId` (`rni-<16 hex>`).
    pub ip_id: String,
    pub subnet_id: String,
    pub ip: Option<String>,
    pub ipv6: Option<String>,
    /// The transient `IpAddressStatus` this IP reports until [`Self::attached_at`]
    /// falls due.
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `ATTACHED` is owed.
    pub attached_at: Instant,
}

impl IpAddressRecord {
    /// This IP's `IpAddressStatus` as of `now`: the transient status until the
    /// attach the clock owes falls due, `ATTACHED` after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.attached_at {
            &self.status
        } else {
            IP_STATUS_ATTACHED
        }
    }

    /// Whether this is the address an `IpAddressUpdate` NAMES, as
    /// `DisassociateResolverEndpointIpAddress` reads one: its `IpId` identifies
    /// an address on its own, and otherwise the address literal does, narrowed
    /// by `SubnetId` when the request carries one. A request that carries only a
    /// subnet names the address stored in that subnet.
    pub fn is_named_by(&self, requested: &IpAddressUpdate) -> bool {
        if let Some(ip_id) = &requested.ip_id {
            return &self.ip_id == ip_id;
        }
        let subnet_matches = requested
            .subnet_id
            .as_ref()
            .is_none_or(|subnet_id| &self.subnet_id == subnet_id);
        if let Some(ip) = &requested.ip {
            return subnet_matches && self.ip.as_ref() == Some(ip);
        }
        if let Some(ipv6) = &requested.ipv6 {
            return subnet_matches && self.ipv6.as_ref() == Some(ipv6);
        }
        requested.subnet_id.is_some() && subnet_matches
    }

    /// Whether this stored address IS the address an `IpAddressUpdate` carries,
    /// which is what `AssociateResolverEndpointIpAddress` consults before adding
    /// one: an IP address is unique within the VPC the endpoint is in, so one
    /// the endpoint already holds cannot be added again. Unlike
    /// [`Self::is_named_by`] the subnet alone decides nothing — an endpoint may
    /// hold several addresses in one subnet.
    pub fn holds_same_address(&self, requested: &IpAddressUpdate) -> bool {
        (requested.ip.is_some() && self.ip == requested.ip)
            || (requested.ipv6.is_some() && self.ipv6 == requested.ipv6)
    }

    /// Project the record onto the modelled `IpAddressResponse`.
    pub fn to_response(&self, now: Instant) -> IpAddressResponse {
        IpAddressResponse {
            ip_id: Some(self.ip_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            ip: self.ip.clone(),
            ipv6: self.ipv6.clone(),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
        }
    }
}

/// A Resolver endpoint, as stored. `IpAddressCount` is DERIVED from
/// [`Self::ip_addresses`] and never held separately, so the count and the
/// addresses cannot disagree.
#[derive(Debug, Clone)]
pub struct ResolverEndpointRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It
    /// is the list order and the source of the minted ids.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub creator_request_id: String,
    pub security_group_ids: Vec<String>,
    pub direction: String,
    pub host_vpc_id: Option<String>,
    /// The transient `ResolverEndpointStatus` reported until [`Self::ready_at`]
    /// falls due (`CREATING` after a create, `UPDATING` after an update).
    pub status: String,
    pub status_message: Option<String>,
    pub endpoint_type: Option<String>,
    pub protocols: Vec<String>,
    pub outpost_arn: Option<String>,
    pub preferred_instance_type: Option<String>,
    pub rni_enhanced_metrics_enabled: Option<bool>,
    pub target_name_server_metrics_enabled: Option<bool>,
    pub dns64_enabled: Option<bool>,
    pub ipv6_internet_access_enabled: Option<bool>,
    pub ip_addresses: Vec<IpAddressRecord>,
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `OPERATIONAL` is owed.
    pub ready_at: Instant,
}

impl ResolverEndpointRecord {
    /// This endpoint's `ResolverEndpointStatus` as of `now`: the transient
    /// status until the transition the clock owes falls due, `OPERATIONAL`
    /// after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.ready_at {
            &self.status
        } else {
            ENDPOINT_STATUS_OPERATIONAL
        }
    }

    /// `IpAddressCount`: the number of IP addresses stored for this endpoint.
    pub fn ip_address_count(&self) -> i32 {
        i32::try_from(self.ip_addresses.len()).unwrap_or(i32::MAX)
    }

    /// Project the record onto the modelled `ResolverEndpoint`.
    pub fn to_endpoint(&self, now: Instant) -> ResolverEndpoint {
        ResolverEndpoint {
            id: Some(self.id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            security_group_ids: Some(self.security_group_ids.clone()),
            direction: Some(self.direction.clone()),
            ip_address_count: Some(self.ip_address_count()),
            host_vpc_id: self.host_vpc_id.clone(),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            outpost_arn: self.outpost_arn.clone(),
            preferred_instance_type: self.preferred_instance_type.clone(),
            resolver_endpoint_type: self.endpoint_type.clone(),
            protocols: Some(self.protocols.clone()),
            rni_enhanced_metrics_enabled: self.rni_enhanced_metrics_enabled,
            target_name_server_metrics_enabled: self.target_name_server_metrics_enabled,
            dns64_enabled: self.dns64_enabled,
            ipv6_internet_access_enabled: self.ipv6_internet_access_enabled,
        }
    }

    /// Whether this endpoint satisfies one `ListResolverEndpoints` filter: does
    /// any value it presents for `key` appear in `values`?
    ///
    /// A member that holds several values (`SecurityGroupIds`) matches when ANY
    /// of them is among the filter values. The test allocates nothing per record
    /// (bar the fixed-size decimal rendering of `IpAddressCount`) and is a set
    /// lookup rather than a scan of the filter values, so the cost of a filtered
    /// page is O(stored endpoints x filter keys) — not multiplied by how many
    /// values the request carried.
    pub fn matches_filter(&self, key: FilterKey, values: &HashSet<String>, now: Instant) -> bool {
        match key {
            FilterKey::CreatorRequestId => values.contains(self.creator_request_id.as_str()),
            FilterKey::Direction => values.contains(self.direction.as_str()),
            FilterKey::HostVpcId => self
                .host_vpc_id
                .as_deref()
                .is_some_and(|id| values.contains(id)),
            FilterKey::IpAddressCount => values.contains(&self.ip_address_count().to_string()),
            FilterKey::Name => self.name.as_deref().is_some_and(|name| values.contains(name)),
            FilterKey::SecurityGroupIds => self
                .security_group_ids
                .iter()
                .any(|group| values.contains(group.as_str())),
            FilterKey::Status => values.contains(self.status_at(now)),
        }
    }
}

/// Fold a wire filter name for matching against a catalog.
///
/// `Filter$Name`'s doc says the early, upper-snake spellings
/// (`CREATOR_REQUEST_ID` for `CreatorRequestId`) "are still supported", so the
/// name is upper-cased with `_` removed before it is matched — which accepts
/// both spellings and nothing else. Written once here because the rule is the
/// same for every list operation's catalog, while the catalogs themselves
/// differ.
fn fold_filter_name(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_uppercase)
        .collect()
}

/// The member a `ListResolverEndpoints` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverEndpoints**): `CreatorRequestId`, `Direction`, `HostVPCId`,
/// `IpAddressCount`, `Name`, `SecurityGroupIds`, `Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKey {
    CreatorRequestId,
    Direction,
    HostVpcId,
    IpAddressCount,
    Name,
    SecurityGroupIds,
    Status,
}

impl FilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreatorRequestId => "CreatorRequestId",
            Self::Direction => "Direction",
            Self::HostVpcId => "HostVPCId",
            Self::IpAddressCount => "IpAddressCount",
            Self::Name => "Name",
            Self::SecurityGroupIds => "SecurityGroupIds",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], which is what
    /// accepts the early upper-snake spelling as well as the documented one.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DIRECTION" => Some(Self::Direction),
            "HOSTVPCID" => Some(Self::HostVpcId),
            "IPADDRESSCOUNT" => Some(Self::IpAddressCount),
            "NAME" => Some(Self::Name),
            "SECURITYGROUPIDS" => Some(Self::SecurityGroupIds),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// A Resolver on an Outpost, as stored.
#[derive(Debug, Clone)]
pub struct OutpostResolverRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub creator_request_id: String,
    pub instance_count: i32,
    pub preferred_instance_type: String,
    pub outpost_arn: String,
    /// The transient `OutpostResolverStatus` reported until [`Self::ready_at`]
    /// falls due (`CREATING` after a create, `UPDATING` after an update).
    pub status: String,
    pub status_message: Option<String>,
    /// `CreateOutpostResolver$Tags`, kept with the record so it is not silently
    /// discarded. `OutpostResolver` models no `Tags` member, so no response
    /// reports them yet: `ListTagsForResource` is still a scaffold stub.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `OPERATIONAL` is owed.
    pub ready_at: Instant,
}

impl OutpostResolverRecord {
    /// This Resolver's `OutpostResolverStatus` as of `now`: the transient
    /// status until the transition the clock owes falls due, `OPERATIONAL`
    /// after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.ready_at {
            &self.status
        } else {
            OUTPOST_RESOLVER_STATUS_OPERATIONAL
        }
    }

    /// Project the record onto the modelled `OutpostResolver`.
    pub fn to_outpost_resolver(&self, now: Instant) -> OutpostResolver {
        OutpostResolver {
            arn: Some(self.arn.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            id: Some(self.id.clone()),
            instance_count: Some(self.instance_count),
            preferred_instance_type: Some(self.preferred_instance_type.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            outpost_arn: Some(self.outpost_arn.clone()),
        }
    }
}

/// A Resolver rule, as stored.
#[derive(Debug, Clone)]
pub struct ResolverRuleRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`].
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub creator_request_id: String,
    pub name: Option<String>,
    pub domain_name: Option<String>,
    pub rule_type: String,
    pub target_ips: Vec<TargetAddress>,
    pub resolver_endpoint_id: Option<String>,
    pub delegation_record: Option<String>,
    pub owner_id: String,
    pub share_status: String,
    /// The `ResolverRuleStatus` this rule reports; see [`RULE_STATUS_COMPLETE`]
    /// on why a stored rule is already settled.
    pub status: String,
    pub status_message: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The resource policy attached to this rule, as the last
    /// `PutResolverRulePolicy` for it wrote it, and `None` while no policy has
    /// been attached.
    ///
    /// Held on the rule's own record, not in a table beside it, because a
    /// resource policy belongs to the resource it shares: a read of the rule's
    /// policy resolves through the same record every other rule operation
    /// reads, and a rule the store no longer holds has no policy to report.
    pub resolver_rule_policy: Option<String>,
}

impl ResolverRuleRecord {
    /// Project the record onto the modelled `ResolverRule`.
    pub fn to_rule(&self) -> ResolverRule {
        ResolverRule {
            id: Some(self.id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            arn: Some(self.arn.clone()),
            domain_name: self.domain_name.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            rule_type: Some(self.rule_type.clone()),
            name: self.name.clone(),
            // `TargetIps` is modelled with a minimum length of 1, so an empty
            // list is left unset rather than serialized as `[]`.
            target_ips: if self.target_ips.is_empty() {
                None
            } else {
                Some(self.target_ips.clone())
            },
            resolver_endpoint_id: self.resolver_endpoint_id.clone(),
            owner_id: Some(self.owner_id.clone()),
            share_status: Some(self.share_status.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            delegation_record: self.delegation_record.clone(),
        }
    }

    /// Whether this rule satisfies one `ListResolverRules` filter: whether the
    /// value it presents for that member is among the filter's values.
    ///
    /// A set lookup per record and per filter key, as
    /// [`ResolverEndpointRecord::matches_filter`] is, so a filtered page costs
    /// O(stored rules x filter keys) and not O(values the request carried). A
    /// member the rule does not carry matches nothing.
    pub fn matches_filter(&self, key: RuleFilterKey, values: &HashSet<String>) -> bool {
        match key {
            RuleFilterKey::CreatorRequestId => values.contains(self.creator_request_id.as_str()),
            RuleFilterKey::DomainName => self
                .domain_name
                .as_deref()
                .is_some_and(|domain| values.contains(domain)),
            RuleFilterKey::Name => self.name.as_deref().is_some_and(|name| values.contains(name)),
            RuleFilterKey::ResolverEndpointId => self
                .resolver_endpoint_id
                .as_deref()
                .is_some_and(|id| values.contains(id)),
            RuleFilterKey::Status => values.contains(self.status.as_str()),
            RuleFilterKey::Type => values.contains(self.rule_type.as_str()),
        }
    }
}

/// The member a `ListResolverRules` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverRules**): `CreatorRequestId`, `DomainName`, `Name`,
/// `ResolverEndpointId`, `Status`, `Type`. It is a different catalog from
/// [`FilterKey`]'s, because the valid filter names "depend on the action that
/// you're including the filter in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleFilterKey {
    CreatorRequestId,
    DomainName,
    Name,
    ResolverEndpointId,
    Status,
    Type,
}

impl RuleFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreatorRequestId => "CreatorRequestId",
            Self::DomainName => "DomainName",
            Self::Name => "Name",
            Self::ResolverEndpointId => "ResolverEndpointId",
            Self::Status => "Status",
            Self::Type => "Type",
        }
    }

    /// Whether the values of this filter name a rule domain, which is matched in
    /// the canonical fully-qualified form a rule is stored and reported in (see
    /// [`canonical_domain_name`]): the doc asks a caller to "include a trailing
    /// dot (.) after the domain name", and canonicalising the value makes both
    /// spellings of one domain select the same rules.
    pub fn selects_on_domain_name(self) -> bool {
        matches!(self, Self::DomainName)
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`FilterKey::parse`] folds the endpoint catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DOMAINNAME" => Some(Self::DomainName),
            "NAME" => Some(Self::Name),
            "RESOLVERENDPOINTID" => Some(Self::ResolverEndpointId),
            "STATUS" => Some(Self::Status),
            "TYPE" => Some(Self::Type),
            _ => None,
        }
    }
}

/// One association between a Resolver rule and a VPC, as stored.
#[derive(Debug, Clone)]
pub struct ResolverRuleAssociationRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`].
    pub seq: u64,
    pub id: String,
    pub resolver_rule_id: String,
    pub vpc_id: String,
    pub name: Option<String>,
    /// The transient `ResolverRuleAssociationStatus` reported until
    /// [`Self::complete_at`] falls due.
    pub status: String,
    pub status_message: Option<String>,
    /// The instant at which the move to `COMPLETE` is owed.
    pub complete_at: Instant,
}

impl ResolverRuleAssociationRecord {
    /// This association's `ResolverRuleAssociationStatus` as of `now`: the
    /// transient status until the transition the clock owes falls due,
    /// `COMPLETE` after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.complete_at {
            &self.status
        } else {
            RULE_ASSOCIATION_STATUS_COMPLETE
        }
    }

    /// Project the record onto the modelled `ResolverRuleAssociation`.
    pub fn to_association(&self, now: Instant) -> ResolverRuleAssociation {
        ResolverRuleAssociation {
            id: Some(self.id.clone()),
            resolver_rule_id: Some(self.resolver_rule_id.clone()),
            name: self.name.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
        }
    }

    /// Whether this association satisfies one `ListResolverRuleAssociations`
    /// filter: whether the value it presents for that member is among the
    /// filter's values, as [`ResolverRuleRecord::matches_filter`] decides it for
    /// a rule. A member the association does not carry matches nothing, and the
    /// status it is matched on is the one it reports as of `now`.
    pub fn matches_filter(
        &self,
        key: RuleAssociationFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            RuleAssociationFilterKey::Name => {
                self.name.as_deref().is_some_and(|name| values.contains(name))
            }
            RuleAssociationFilterKey::ResolverRuleId => {
                values.contains(self.resolver_rule_id.as_str())
            }
            RuleAssociationFilterKey::Status => values.contains(self.status_at(now)),
            RuleAssociationFilterKey::VpcId => values.contains(self.vpc_id.as_str()),
        }
    }
}

/// The member a `ListResolverRuleAssociations` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverRuleAssociations**): `Name`, `ResolverRuleId`, `Status`,
/// `VPCId`. It is a different catalog from [`RuleFilterKey`]'s, because the
/// valid filter names "depend on the action that you're including the filter
/// in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAssociationFilterKey {
    Name,
    ResolverRuleId,
    Status,
    VpcId,
}

impl RuleAssociationFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::ResolverRuleId => "ResolverRuleId",
            Self::Status => "Status",
            Self::VpcId => "VPCId",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`RuleFilterKey::parse`] folds the rule catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "NAME" => Some(Self::Name),
            "RESOLVERRULEID" => Some(Self::ResolverRuleId),
            "STATUS" => Some(Self::Status),
            "VPCID" => Some(Self::VpcId),
            _ => None,
        }
    }
}

/// `Validation` values, the model's whole enum
/// (`smithy_query(query="shape",shape="Validation")`, re-exported as
/// [`crate::ops::types::VALIDATION_VALUES`]): the new DNSSEC validation setting
/// a request asks a VPC's configuration to take.
pub const VALIDATION_ENABLE: &str = "ENABLE";
pub const VALIDATION_DISABLE: &str = "DISABLE";
pub const VALIDATION_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// `ResolverDNSSECValidationStatus` values the DNSSEC validation slice reports,
/// the model's whole enum
/// (`smithy_query(query="shape",shape="ResolverDNSSECValidationStatus")`,
/// re-exported as
/// [`crate::ops::types::RESOLVER_D_N_S_S_E_C_VALIDATION_STATUS_VALUES`]).
///
/// `ValidationStatus`' doc pairs each settled value with the transient one that
/// precedes it: "ENABLING: DNSSEC validation is being enabled but is not
/// complete", "ENABLED: DNSSEC validation is enabled", "DISABLING: DNSSEC
/// validation is being disabled but is not complete", "DISABLED: DNSSEC
/// validation is disabled".
pub const DNSSEC_STATUS_ENABLING: &str = "ENABLING";
pub const DNSSEC_STATUS_ENABLED: &str = "ENABLED";
pub const DNSSEC_STATUS_DISABLING: &str = "DISABLING";
pub const DNSSEC_STATUS_DISABLED: &str = "DISABLED";
pub const DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING: &str =
    "UPDATING_TO_USE_LOCAL_RESOURCE_SETTING";
pub const DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// The most configurations for DNSSEC validation this store holds records for,
/// for the same reason as [`MAX_STORED_ENDPOINTS`]: every write operation of
/// this service is reachable by any caller, so the store must not grow without
/// a bound. One record is allocated per distinct `ResourceId` whose validation
/// setting is moved off its default, and an update of a `ResourceId` already
/// stored stays allowed, because it allocates nothing.
pub const MAX_STORED_DNSSEC_CONFIGS: usize = 1_000;

/// The pair of `ResolverDNSSECValidationStatus` values one requested
/// `Validation` moves a VPC's configuration through: the transient status it
/// reports while the change is in flight, and the settled status it reports
/// once the clock owes the move.
///
/// `Validation`'s doc — "Be aware that it can take time for a validation status
/// change to be completed" — is why the transient status exists, and
/// `ValidationStatus`' doc is what pairs each settled value with it.
/// `USE_LOCAL_RESOURCE_SETTING` takes the enum's own
/// `UPDATING_TO_USE_LOCAL_RESOURCE_SETTING` as its in-flight value. `None` for
/// a value the model's `Validation` enum does not declare: a handler refuses
/// that rather than treating it as one of these.
pub fn dnssec_validation_transition(validation: &str) -> Option<(&'static str, &'static str)> {
    match validation {
        VALIDATION_ENABLE => Some((DNSSEC_STATUS_ENABLING, DNSSEC_STATUS_ENABLED)),
        VALIDATION_DISABLE => Some((DNSSEC_STATUS_DISABLING, DNSSEC_STATUS_DISABLED)),
        VALIDATION_USE_LOCAL_RESOURCE_SETTING => Some((
            DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
        )),
        _ => None,
    }
}

/// The `Id` of a VPC's configuration for DNSSEC validation. AWS names one
/// `rdsc-<hex>`; every result fits `ResourceId` (length 1..64).
///
/// Derived from the VPC id rather than taken from the id sequence, for the same
/// reason as [`resolver_config_id`]: no operation creates a configuration for
/// DNSSEC validation (`UpdateResolverDnssecConfig` only moves the validation
/// setting of the one a VPC already has), so one exists for a VPC whether or not
/// anything has ever written to it, and its id has to be a function of the VPC
/// it belongs to. The same VPC therefore reports the same `Id` from every call,
/// before and after an update.
pub fn dnssec_config_id(resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/dnssec-config/v1");
    feed_field(&mut hasher, resource_id.as_bytes());
    format!("rdsc-{}", hex::encode(&hasher.finalize()[..8]))
}

/// One configuration for DNSSEC validation, as stored.
///
/// The reported `ValidationStatus` is computed from the stored status pair and
/// the due instant, so a read never has to write to settle one — the same shape
/// as [`ResolverEndpointRecord`].
///
/// A configuration for DNSSEC validation is a per-VPC singleton the service
/// maintains itself — there is no operation that creates or deletes one — so a
/// record exists only for a VPC whose validation setting was moved off its
/// default; every other VPC reports the default one
/// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_dnssec_config`]
/// derives. `Id` and `OwnerId` are therefore the same whether the record exists
/// or not, so a VPC's configuration does not change identity when it is updated
/// — the same shape as [`ResolverConfigRecord`].
#[derive(Debug, Clone)]
pub struct ResolverDnssecConfigRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// order `ListResolverDnssecConfigs` reports the configurations in,
    /// independent of the hash order of the map that holds them.
    ///
    /// `0` on the default configuration
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_dnssec_config`]
    /// derives for a VPC the store holds no record for: the id sequence starts
    /// at 1, so a stored record never carries it.
    pub seq: u64,
    /// The `Id` of this configuration, [`dnssec_config_id`] of its VPC, stable
    /// for the life of the VPC: a later update of the same VPC keeps it.
    pub id: String,
    /// `OwnerId`: "The owner account ID of the virtual private cloud (VPC)".
    pub owner_id: String,
    /// `ResourceId`: the VPC this configuration is for.
    pub resource_id: String,
    /// The `ValidationStatus` reported while the requested change is in flight.
    pub status: String,
    /// The `ValidationStatus` reported once the move the clock owes falls due.
    pub settled_status: String,
    /// When that move falls due.
    pub settled_at: Instant,
}

impl ResolverDnssecConfigRecord {
    /// This configuration's `ValidationStatus` as of `now`: the transient
    /// status until the move the clock owes falls due, the settled one after
    /// it.
    pub fn validation_status_at(&self, now: Instant) -> &str {
        if now < self.settled_at {
            &self.status
        } else {
            &self.settled_status
        }
    }

    /// Project the record onto the modelled `ResolverDnssecConfig`.
    pub fn to_dnssec_config(&self, now: Instant) -> ResolverDnssecConfig {
        ResolverDnssecConfig {
            id: Some(self.id.clone()),
            owner_id: Some(self.owner_id.clone()),
            resource_id: Some(self.resource_id.clone()),
            validation_status: Some(self.validation_status_at(now).to_string()),
        }
    }

    /// Whether this configuration satisfies one `ListResolverDnssecConfigs`
    /// filter: does the value it presents for `key` appear in `values`?
    pub fn matches_filter(
        &self,
        key: DnssecConfigFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            DnssecConfigFilterKey::Id => values.contains(self.id.as_str()),
            DnssecConfigFilterKey::OwnerId => values.contains(self.owner_id.as_str()),
            DnssecConfigFilterKey::ResourceId => values.contains(self.resource_id.as_str()),
            DnssecConfigFilterKey::ValidationStatus => values.contains(self.validation_status_at(now)),
        }
    }
}

/// The member a `ListResolverDnssecConfigs` filter selects on.
///
/// `Filter$Name`'s doc ("The valid values for `Name` depend on the action that
/// you're including the filter in") names no catalog for this operation, so the
/// catalog is the members the listed shape presents
/// (`smithy_query(query="shape",shape="ResolverDnssecConfig")`): `Id`,
/// `OwnerId`, `ResourceId`, `ValidationStatus`. A name outside it is refused
/// rather than silently ignored, so a caller is never handed a page that does
/// not answer the query it asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnssecConfigFilterKey {
    Id,
    OwnerId,
    ResourceId,
    ValidationStatus,
}

impl DnssecConfigFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Id => "Id",
            Self::OwnerId => "OwnerId",
            Self::ResourceId => "ResourceId",
            Self::ValidationStatus => "ValidationStatus",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the catalog.
    /// The name is folded by [`fold_filter_name`], as [`FilterKey::parse`] folds
    /// the endpoint catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "ID" => Some(Self::Id),
            "OWNERID" => Some(Self::OwnerId),
            "RESOURCEID" => Some(Self::ResourceId),
            "VALIDATIONSTATUS" => Some(Self::ValidationStatus),
            _ => None,
        }
    }
}

/// The cursor scope of one `ListResolverDnssecConfigs` query, for the same
/// reason and in the same shape as [`endpoint_list_scope`]'s.
pub fn dnssec_config_list_scope(filters: &[(DnssecConfigFilterKey, HashSet<String>)]) -> String {
    filter_list_scope(
        "ListResolverDnssecConfigs",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// `AutodefinedReverseFlag` values, the model's whole enum
/// (`smithy_query(query="shape",shape="AutodefinedReverseFlag")`, re-exported as
/// [`crate::ops::types::AUTODEFINED_REVERSE_FLAG_VALUES`]): the setting a
/// request asks a VPC's Resolver configuration to take for the autodefined
/// rules for reverse DNS lookups.
pub const AUTODEFINED_REVERSE_FLAG_ENABLE: &str = "ENABLE";
pub const AUTODEFINED_REVERSE_FLAG_DISABLE: &str = "DISABLE";
pub const AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// `ResolverAutodefinedReverseStatus` values a VPC's Resolver configuration
/// reports, the model's whole enum
/// (`smithy_query(query="shape",shape="ResolverAutodefinedReverseStatus")`,
/// re-exported as
/// [`crate::ops::types::RESOLVER_AUTODEFINED_REVERSE_STATUS_VALUES`]).
///
/// `ResolverConfig$AutodefinedReverse`'s doc pairs each settled value with the
/// transient one that precedes it: "ENABLING: Autodefined rules for reverse DNS
/// lookups are being enabled but are not complete", "ENABLED: Autodefined rules
/// for reverse DNS lookups are enabled", "DISABLING: Autodefined rules for
/// reverse DNS lookups are being disabled but are not complete", "DISABLED:
/// Autodefined rules for reverse DNS lookups are disabled".
pub const AUTODEFINED_REVERSE_ENABLING: &str = "ENABLING";
pub const AUTODEFINED_REVERSE_ENABLED: &str = "ENABLED";
pub const AUTODEFINED_REVERSE_DISABLING: &str = "DISABLING";
pub const AUTODEFINED_REVERSE_DISABLED: &str = "DISABLED";
pub const AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING: &str =
    "UPDATING_TO_USE_LOCAL_RESOURCE_SETTING";
pub const AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// `LimitExceededException$ResourceType` for a VPC's Resolver behavior
/// configuration: the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_RESOLVER_CONFIG: &str = "ResolverConfig";

/// The most VPC Resolver configurations this store holds records for, for the
/// same reason as [`MAX_STORED_FIREWALL_CONFIGS`]: `UpdateResolverConfig`
/// admits any `ResourceId`, so without a bound a caller looping updates over
/// fresh VPC ids would grow one emulator process without limit. An update that
/// would need a record past this count is refused and writes nothing; one for a
/// VPC the store already holds a record for allocates nothing and stays
/// allowed, and so does a read of any VPC, which stores nothing at all.
pub const MAX_STORED_RESOLVER_CONFIGS: usize = 1_000;

/// The pair of `ResolverAutodefinedReverseStatus` values one requested
/// `AutodefinedReverseFlag` moves a VPC's Resolver configuration through: the
/// transient status it reports while the change is in flight, and the settled
/// status it reports once the clock owes the move.
///
/// `AutodefinedReverseFlag`'s doc — "It can take some time for the status change
/// to be completed" — is why the transient status exists, and
/// `ResolverConfig$AutodefinedReverse`'s doc is what pairs each settled value
/// with it. `USE_LOCAL_RESOURCE_SETTING` takes the status enum's own
/// `UPDATING_TO_USE_LOCAL_RESOURCE_SETTING` as its in-flight value. `None` for a
/// value the model's `AutodefinedReverseFlag` enum does not declare: a handler
/// refuses that rather than treating it as one of these.
pub fn autodefined_reverse_transition(flag: &str) -> Option<(&'static str, &'static str)> {
    match flag {
        AUTODEFINED_REVERSE_FLAG_ENABLE => {
            Some((AUTODEFINED_REVERSE_ENABLING, AUTODEFINED_REVERSE_ENABLED))
        }
        AUTODEFINED_REVERSE_FLAG_DISABLE => {
            Some((AUTODEFINED_REVERSE_DISABLING, AUTODEFINED_REVERSE_DISABLED))
        }
        AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING => Some((
            AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING,
        )),
        _ => None,
    }
}

/// The Resolver behavior configuration of ONE VPC, as stored.
///
/// A VPC's Resolver configuration is a singleton the service maintains itself —
/// there is no operation that creates or deletes one — so a record exists only
/// for a VPC whose configuration was moved off its default; every other VPC
/// reports the default one
/// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_config`]
/// derives. `Id` and `OwnerId` are therefore the same whether the record exists
/// or not, so a VPC's configuration does not change identity when it is
/// updated — the same shape as [`FirewallConfigRecord`].
#[derive(Debug, Clone)]
pub struct ResolverConfigRecord {
    /// Creation order, for the list operation's pages (the same role as
    /// [`FirewallConfigRecord::seq`]).
    ///
    /// `0` on the default configuration
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_config`]
    /// derives for a VPC the store holds no record for: the id sequence starts
    /// at 1, so a stored record never carries it.
    pub seq: u64,
    /// The `Id` of this configuration, [`resolver_config_id`] of its VPC.
    pub id: String,
    /// `ResourceId`: "The ID of the Amazon Virtual Private Cloud VPC or a Route
    /// 53 Profile that you're configuring Resolver for."
    pub resource_id: String,
    /// `OwnerId`: "The owner account ID of the Amazon Virtual Private Cloud
    /// VPC."
    pub owner_id: String,
    /// The `AutodefinedReverse` status reported while the requested change is
    /// in flight.
    pub status: String,
    /// The `AutodefinedReverse` status reported once the move the clock owes
    /// falls due.
    pub settled_status: String,
    /// When that move falls due.
    pub settled_at: Instant,
}

impl ResolverConfigRecord {
    /// This configuration's `AutodefinedReverse` as of `now`: the transient
    /// status until the move the clock owes falls due, the settled one after
    /// it. Computed, so a read never has to write to settle one.
    pub fn autodefined_reverse_at(&self, now: Instant) -> &str {
        if now < self.settled_at {
            &self.status
        } else {
            &self.settled_status
        }
    }

    /// Project the record onto the modelled `ResolverConfig` shape the Resolver
    /// configuration operations report.
    pub fn to_resolver_config(&self, now: Instant) -> ResolverConfig {
        ResolverConfig {
            id: Some(self.id.clone()),
            resource_id: Some(self.resource_id.clone()),
            owner_id: Some(self.owner_id.clone()),
            autodefined_reverse: Some(self.autodefined_reverse_at(now).to_string()),
        }
    }
}

/// `ResolverQueryLogConfigStatus` values this slice reports, out of the model's
/// enum (`smithy_query(query="shape",shape="ResolverQueryLogConfigStatus")`:
/// `CREATING | CREATED | DELETING | FAILED`).
///
/// `CREATING` is "Resolver is creating the query logging configuration", which
/// is what a create answers with, and `CREATED` is "the query logging
/// configuration was successfully created": the move between them is the one the
/// clock owes, [`PROVISIONING_DELAY`] after the call that started it. `DELETING`
/// describes a configuration a delete has been accepted for and `FAILED` one
/// whose destination Resolver cannot deliver to, neither of which a create
/// produces.
pub const QUERY_LOG_CONFIG_STATUS_CREATING: &str = "CREATING";

/// See [`QUERY_LOG_CONFIG_STATUS_CREATING`].
pub const QUERY_LOG_CONFIG_STATUS_CREATED: &str = "CREATED";

/// `ResourceNotFoundException$ResourceType` for a query logging configuration:
/// the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG: &str = "ResolverQueryLogConfig";

/// `ResolverQueryLogConfigAssociationStatus` values this slice reports, out of
/// the model's enum
/// (`smithy_query(query="shape",shape="ResolverQueryLogConfigAssociationStatus")`,
/// re-exported as
/// [`crate::ops::types::RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION_STATUS_VALUES`]).
///
/// `CREATING` is "Resolver is creating an association between an Amazon VPC and
/// a query logging configuration", which is what an associate answers with, and
/// `ACTIVE` is "the association ... was successfully created": the move between
/// them is the one the clock owes, [`PROVISIONING_DELAY`] after the call that
/// started it. `DELETING`, `ACTION_NEEDED` and `FAILED` describe an association
/// a disassociate has been accepted for, or one Resolver could not create or
/// delete, neither of which an associate produces.
pub const QUERY_LOG_CONFIG_ASSOCIATION_STATUS_CREATING: &str = "CREATING";

/// See [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_CREATING`].
pub const QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `ResourceNotFoundException$ResourceType` for one association between a VPC
/// and a query logging configuration: the modelled shape name of the resource,
/// as for [`RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG`].
pub const RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION: &str =
    "ResolverQueryLogConfigAssociation";

/// The most VPC associations one query logging configuration holds, for the same
/// reason as [`MAX_STORED_QUERY_LOG_CONFIGS`]: `AssociateResolverQueryLogConfig`
/// admits any VPC the configuration is not already associated with, so without a
/// bound a caller looping associates would grow one emulator process without
/// limit. An associate past this count is REFUSED by
/// [`ResolverQueryLogConfigRecord::can_hold_another_association`] with the
/// `LimitExceededException` the operation models, and a disassociate frees a
/// slot.
pub const MAX_QUERY_LOG_CONFIG_ASSOCIATIONS: usize = 1_000;

/// The most query logging configurations this store holds, for the same reason
/// as [`MAX_STORED_ENDPOINTS`]: `CreateResolverQueryLogConfig` admits any unseen
/// `CreatorRequestId`, so without a bound a caller looping creates would grow
/// one emulator process without limit. A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_query_log_config`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record.
pub const MAX_STORED_QUERY_LOG_CONFIGS: usize = 1_000;

/// The destinations Resolver delivers query logs to, as
/// `CreateResolverQueryLogConfig$DestinationArn` documents them — "an S3 bucket,
/// a CloudWatch Logs log group, or a Kinesis Data Firehose delivery stream" —
/// each as the `service` field its ARN carries together with the prefix that
/// field's resource part must start with, from the examples the same doc gives
/// (`arn:aws:s3:::amzn-s3-demo-bucket`,
/// `arn:aws:logs:us-west-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*`,
/// `arn:aws:kinesis:us-east-2:0123456789:stream/my_stream_name`). A Firehose
/// delivery stream also carries its own `firehose` service with a
/// `deliverystream/` resource.
///
/// The third field is the name that kind of destination is selected by, from the
/// `Destination` filter `ListResolverQueryLogConfigs` documents
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigs**: `S3`, `CloudWatchLogs`, `KinesisFirehose`),
/// so one table states both which destinations are well formed and what names
/// each of them.
const QUERY_LOG_DESTINATIONS: [(&str, &str, &str); 4] = [
    ("s3", "", "S3"),
    ("logs", "log-group:", "CloudWatchLogs"),
    ("kinesis", "stream/", "KinesisFirehose"),
    ("firehose", "deliverystream/", "KinesisFirehose"),
];

/// Whether `destination_arn` names one of the destinations Resolver can send
/// query logs to, i.e. is an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`) whose service and
/// resource are one of [`QUERY_LOG_DESTINATIONS`].
///
/// The model constrains `DestinationArn` only to length 1..600, so a string that
/// names no such destination reaches the handler, which refuses it with the
/// client error the operation models rather than storing a configuration whose
/// logs could never be delivered.
pub fn query_log_destination_is_wellformed(destination_arn: &str) -> bool {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    let [prefix, partition, service, _region, _account, resource] = fields.as_slice() else {
        return false;
    };
    if *prefix != "arn" || partition.is_empty() || resource.is_empty() {
        return false;
    }
    QUERY_LOG_DESTINATIONS
        .iter()
        .any(|(name, resource_prefix, _)| *name == *service && resource.starts_with(resource_prefix))
}

/// The Amazon Web Services service a stored `DestinationArn` delivers to, by the
/// name the `Destination` filter selects it with (see
/// [`QUERY_LOG_DESTINATIONS`]), or `None` for an ARN that names no destination
/// Resolver delivers to.
pub fn query_log_destination_kind(destination_arn: &str) -> Option<&'static str> {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    let [_, _, service, _, _, resource] = fields.as_slice() else {
        return None;
    };
    QUERY_LOG_DESTINATIONS
        .iter()
        .find(|(name, resource_prefix, _)| {
            *name == *service && resource.starts_with(resource_prefix)
        })
        .map(|(_, _, kind)| *kind)
}

/// The form a query logging destination is STORED in.
///
/// A CloudWatch Logs log group is addressed by an ARN that may carry the `:*`
/// log-stream wildcard
/// (`arn:<partition>:logs:<region>:<account>:log-group:<name>:*`, the form
/// `CreateResolverQueryLogConfig$DestinationArn`'s own example uses). That
/// wildcard names every stream in the group rather than part of the group's
/// identity, and the destination is the GROUP, so it is dropped and every read
/// of the configuration reports the canonical log-group ARN. Every other
/// destination is stored exactly as the request wrote it.
pub fn canonical_query_log_destination(destination_arn: &str) -> String {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    if let [_, _, service, _, _, resource] = fields.as_slice() {
        if *service == "logs" && resource.starts_with("log-group:") {
            if let Some(group) = destination_arn.strip_suffix(":*") {
                return group.to_string();
            }
        }
    }
    destination_arn.to_string()
}

/// The `InvalidParameterException` a query-logging write answers for a
/// `DestinationArn` that names none of the destinations Resolver delivers to.
///
/// `InvalidParameterException` is "one or more parameters in this request are not
/// valid" and the model gives it a `FieldName`, "the parameter that is not
/// valid", so the refusal names the member that was rejected.
pub fn invalid_query_log_destination(destination_arn: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: format!(
            "Invalid destination ARN \"{destination_arn}\": query logs can be sent only to an \
             S3 bucket, a CloudWatch Logs log group or a Kinesis Data Firehose delivery stream."
        ),
        field_name: Some("DestinationArn".to_string()),
    }
}

/// The `InvalidParameterException` a query-logging write answers for a `Name`
/// the modelled pattern excludes but the boundary validator cannot refuse; see
/// [`name_satisfies_pattern`] on why it reaches a handler at all.
pub fn invalid_query_log_config_name(name: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: format!(
            "Invalid name \"{name}\": a query logging configuration name must carry at least one \
             letter, digit, hyphen, underscore, apostrophe or space, and cannot consist of only \
             digits."
        ),
        field_name: Some("Name".to_string()),
    }
}

/// The `ResourceNotFoundException` the query-logging operations answer for an id
/// they hold no configuration under, built here so every operation that resolves
/// a `ResolverQueryLogConfigId` reports the same body. See
/// [`outpost_resolver_not_found`] on the absent `RSLVR-nnnnn` service error code.
pub fn resolver_query_log_config_not_found(
    resolver_query_log_config_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            r#"Resolver query log config with ID "{resolver_query_log_config_id}" does not exist."#
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG.to_string()),
    }
}

/// The `ResourceNotFoundException` the query-logging operations answer for an
/// association id they hold no association under, built here so every operation
/// that resolves a `ResolverQueryLogConfigAssociationId` reports the same body.
pub fn resolver_query_log_config_association_not_found(
    resolver_query_log_config_association_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            r#"Resolver query log config association with ID "{resolver_query_log_config_association_id}" does not exist."#
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION.to_string()),
    }
}

/// One association between a VPC and a Resolver query logging configuration, as
/// stored. It lives on the configuration it belongs to, so the configuration's
/// `AssociationCount` and the associations it holds cannot disagree.
#[derive(Debug, Clone)]
pub struct ResolverQueryLogConfigAssociationRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// association order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub resolver_query_log_config_id: String,
    /// The VPC "that you want this query logging configuration to log queries
    /// for".
    pub resource_id: String,
    /// The transient `ResolverQueryLogConfigAssociationStatus` reported until
    /// [`Self::active_at`] falls due.
    pub status: String,
    pub creation_time: String,
    /// The instant at which the move to
    /// [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE`] is owed.
    pub active_at: Instant,
}

impl ResolverQueryLogConfigAssociationRecord {
    /// This association's `ResolverQueryLogConfigAssociationStatus` as of `now`:
    /// the transient status until the transition the clock owes falls due,
    /// [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE`] after it. Computed, so a
    /// read never has to write to settle one.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.active_at {
            &self.status
        } else {
            QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE
        }
    }

    /// Project the record onto the modelled `ResolverQueryLogConfigAssociation`.
    ///
    /// `Error` is left unset, and `ErrorMessage` with it: "if the value of
    /// `Status` is a value other than `FAILED`, `Error` is null", and this slice
    /// produces no failed association.
    pub fn to_association(&self, now: Instant) -> ResolverQueryLogConfigAssociation {
        ResolverQueryLogConfigAssociation {
            id: Some(self.id.clone()),
            resolver_query_log_config_id: Some(self.resolver_query_log_config_id.clone()),
            resource_id: Some(self.resource_id.clone()),
            status: Some(self.status_at(now).to_string()),
            error: None,
            error_message: None,
            creation_time: Some(self.creation_time.clone()),
        }
    }
}

/// A Resolver query logging configuration, as stored.
///
/// `AssociationCount` is DERIVED from [`Self::associations`] and never held
/// separately — the same way [`FirewallDomainListRecord::domain_count`] derives
/// from its domains — so the count and the associations cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct ResolverQueryLogConfigRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub owner_id: String,
    pub destination_arn: String,
    pub creator_request_id: String,
    /// The `ShareStatus` this configuration reports; the emulator has one
    /// account, so it is [`SHARE_STATUS_NOT_SHARED`].
    pub share_status: String,
    /// The associations between this configuration and the VPCs it logs queries
    /// for, in association order — what `AssociateResolverQueryLogConfig` adds
    /// to and `DisassociateResolverQueryLogConfig` takes out of. A configuration
    /// is created with none: "to specify which VPCs you want to log queries for,
    /// you use AssociateResolverQueryLogConfig".
    pub associations: Vec<ResolverQueryLogConfigAssociationRecord>,
    /// The `ResolverQueryLogConfigStatus` this configuration reports, as of
    /// [`Self::settled_at`].
    pub status: String,
    /// `CreateResolverQueryLogConfig$Tags`, kept with the record so they are not
    /// silently discarded. `ResolverQueryLogConfig` models no `Tags` member, so
    /// no response reports them: `ListTagsForResource` is a separate operation.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// [`QUERY_LOG_CONFIG_STATUS_CREATED`] is owed, or `None` for a
    /// configuration with no transition in flight.
    pub settled_at: Option<Instant>,
}

impl ResolverQueryLogConfigRecord {
    /// `AssociationCount`: "The number of VPCs that are associated with the
    /// query logging configuration."
    pub fn association_count(&self) -> i32 {
        i32::try_from(self.associations.len()).unwrap_or(i32::MAX)
    }

    /// This configuration's association with the VPC `resource_id`, if it holds
    /// one.
    ///
    /// A configuration and a VPC are associated at most once — "to associate
    /// more than one VPC with a configuration, submit one
    /// AssociateResolverQueryLogConfig request for each VPC" — so this is what
    /// `AssociateResolverQueryLogConfig` consults before writing a second record
    /// for the same pair. The scan is over a list bounded by
    /// [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`].
    pub fn association_for_vpc(
        &self,
        resource_id: &str,
    ) -> Option<&ResolverQueryLogConfigAssociationRecord> {
        self.associations
            .iter()
            .find(|record| record.resource_id == resource_id)
    }

    /// Whether one more VPC may be associated with this configuration, i.e. it
    /// still holds fewer than [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`]. An
    /// associate that this refuses answers `LimitExceededException` and writes
    /// nothing.
    pub fn can_hold_another_association(&self) -> bool {
        self.associations.len() < MAX_QUERY_LOG_CONFIG_ASSOCIATIONS
    }

    /// This configuration's `ResolverQueryLogConfigStatus` as of `now`: the
    /// transient status until the transition the clock owes falls due,
    /// [`QUERY_LOG_CONFIG_STATUS_CREATED`] after it, and the stored status
    /// itself when no transition is owed — the same shape as
    /// [`FirewallDomainListRecord::status_at`].
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            Some(_) => QUERY_LOG_CONFIG_STATUS_CREATED,
            None => &self.status,
        }
    }

    /// Whether this configuration satisfies one `ListResolverQueryLogConfigs`
    /// filter: whether the value it presents for that member is among the
    /// filter's values, as [`ResolverRuleRecord::matches_filter`] decides it for
    /// a rule. The status it is matched on is the one it reports as of `now`, so
    /// a filtered page and a read of the same configuration cannot disagree.
    pub fn matches_filter(
        &self,
        key: QueryLogConfigFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            QueryLogConfigFilterKey::Arn => values.contains(self.arn.as_str()),
            QueryLogConfigFilterKey::AssociationCount => {
                values.contains(&self.association_count().to_string())
            }
            // `Filter$Name`'s `CreationTime` entry spells this member's values
            // "in Unix time format and Coordinated Universal Time (UTC)", while
            // every read reports the modelled RFC 3339 string, so a filter value
            // in either spelling selects the configuration: the whole-second
            // Unix form names the second the configuration was created in.
            QueryLogConfigFilterKey::CreationTime => {
                values.contains(self.creation_time.as_str())
                    || values.contains(&rfc3339_unix_seconds(&self.creation_time))
            }
            QueryLogConfigFilterKey::CreatorRequestId => {
                values.contains(self.creator_request_id.as_str())
            }
            QueryLogConfigFilterKey::Destination => query_log_destination_kind(&self.destination_arn)
                .is_some_and(|kind| values.contains(kind)),
            QueryLogConfigFilterKey::DestinationArn => {
                values.contains(self.destination_arn.as_str())
            }
            QueryLogConfigFilterKey::Id => values.contains(self.id.as_str()),
            QueryLogConfigFilterKey::Name => values.contains(self.name.as_str()),
            QueryLogConfigFilterKey::OwnerId => values.contains(self.owner_id.as_str()),
            QueryLogConfigFilterKey::ShareStatus => values.contains(self.share_status.as_str()),
            QueryLogConfigFilterKey::Status => values.contains(self.status_at(now)),
        }
    }

    /// The value this configuration is ordered by when a request sorts on `key`:
    /// the member's own value, as a number for the two members that are not text
    /// and as text for the rest (see [`QueryLogConfigSortValue`]).
    pub fn sort_value(&self, key: QueryLogConfigSortKey, now: Instant) -> QueryLogConfigSortValue {
        match key {
            QueryLogConfigSortKey::Arn => QueryLogConfigSortValue::Text(self.arn.clone()),
            QueryLogConfigSortKey::AssociationCount => {
                QueryLogConfigSortValue::Number(i128::from(self.association_count()))
            }
            QueryLogConfigSortKey::CreationTime => {
                QueryLogConfigSortValue::Number(rfc3339_epoch_nanos(&self.creation_time))
            }
            QueryLogConfigSortKey::CreatorRequestId => {
                QueryLogConfigSortValue::Text(self.creator_request_id.clone())
            }
            QueryLogConfigSortKey::DestinationArn => {
                QueryLogConfigSortValue::Text(self.destination_arn.clone())
            }
            QueryLogConfigSortKey::Id => QueryLogConfigSortValue::Text(self.id.clone()),
            QueryLogConfigSortKey::Name => QueryLogConfigSortValue::Text(self.name.clone()),
            QueryLogConfigSortKey::OwnerId => QueryLogConfigSortValue::Text(self.owner_id.clone()),
            QueryLogConfigSortKey::ShareStatus => {
                QueryLogConfigSortValue::Text(self.share_status.clone())
            }
            QueryLogConfigSortKey::Status => {
                QueryLogConfigSortValue::Text(self.status_at(now).to_string())
            }
        }
    }

    /// Project the record onto the modelled `ResolverQueryLogConfig`.
    pub fn to_query_log_config(&self, now: Instant) -> ResolverQueryLogConfig {
        ResolverQueryLogConfig {
            id: Some(self.id.clone()),
            owner_id: Some(self.owner_id.clone()),
            status: Some(self.status_at(now).to_string()),
            share_status: Some(self.share_status.clone()),
            association_count: self.association_count(),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            destination_arn: Some(self.destination_arn.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            creation_time: Some(self.creation_time.clone()),
        }
    }
}

/// The member a `ListResolverQueryLogConfigs` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigs**): `Arn`, `AssociationCount`, `CreationTime`,
/// `CreatorRequestId`, `Destination`, `DestinationArn`, `Id`, `Name`, `OwnerId`,
/// `ShareStatus`, `Status`. It is a different catalog from [`FilterKey`]'s,
/// because the valid filter names "depend on the action that you're including
/// the filter in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLogConfigFilterKey {
    Arn,
    AssociationCount,
    CreationTime,
    CreatorRequestId,
    /// "The Amazon Web Services service that you want to forward query logs to",
    /// which is the KIND of the configuration's destination and not its ARN (see
    /// [`query_log_destination_kind`]).
    Destination,
    DestinationArn,
    Id,
    Name,
    OwnerId,
    ShareStatus,
    Status,
}

impl QueryLogConfigFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Arn => "Arn",
            Self::AssociationCount => "AssociationCount",
            Self::CreationTime => "CreationTime",
            Self::CreatorRequestId => "CreatorRequestId",
            Self::Destination => "Destination",
            Self::DestinationArn => "DestinationArn",
            Self::Id => "Id",
            Self::Name => "Name",
            Self::OwnerId => "OwnerId",
            Self::ShareStatus => "ShareStatus",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`FilterKey::parse`] folds the endpoint catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "ARN" => Some(Self::Arn),
            "ASSOCIATIONCOUNT" => Some(Self::AssociationCount),
            "CREATIONTIME" => Some(Self::CreationTime),
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DESTINATION" => Some(Self::Destination),
            "DESTINATIONARN" => Some(Self::DestinationArn),
            "ID" => Some(Self::Id),
            "NAME" => Some(Self::Name),
            "OWNERID" => Some(Self::OwnerId),
            "SHARESTATUS" => Some(Self::ShareStatus),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// The member a `ListResolverQueryLogConfigs` request sorts its configurations
/// by.
///
/// The catalog is exactly the one that operation's `SortBy` documents
/// (`smithy_query(query="doc",shape="ListResolverQueryLogConfigsRequest",member="SortBy")`):
/// `Arn`, `AssociationCount`, `CreationTime`, `CreatorRequestId`,
/// `DestinationArn`, `Id`, `Name`, `OwnerId`, `ShareStatus`, `Status`. It is NOT
/// the filter catalog: `Destination` can be filtered on but not sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLogConfigSortKey {
    Arn,
    AssociationCount,
    CreationTime,
    CreatorRequestId,
    DestinationArn,
    Id,
    Name,
    OwnerId,
    ShareStatus,
    Status,
}

impl QueryLogConfigSortKey {
    /// The canonical name of this key, used to describe one query's ordering in
    /// a pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Arn => "Arn",
            Self::AssociationCount => "AssociationCount",
            Self::CreationTime => "CreationTime",
            Self::CreatorRequestId => "CreatorRequestId",
            Self::DestinationArn => "DestinationArn",
            Self::Id => "Id",
            Self::Name => "Name",
            Self::OwnerId => "OwnerId",
            Self::ShareStatus => "ShareStatus",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire `SortBy`, or `None` when it names no member of the
    /// documented catalog. Folded by [`fold_filter_name`] for the same reason a
    /// filter name is: the service accepts the upper-snake spelling of a member
    /// name as well as the documented one.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "ARN" => Some(Self::Arn),
            "ASSOCIATIONCOUNT" => Some(Self::AssociationCount),
            "CREATIONTIME" => Some(Self::CreationTime),
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DESTINATIONARN" => Some(Self::DestinationArn),
            "ID" => Some(Self::Id),
            "NAME" => Some(Self::Name),
            "OWNERID" => Some(Self::OwnerId),
            "SHARESTATUS" => Some(Self::ShareStatus),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// The value one configuration is ordered by under a `SortBy`.
///
/// `AssociationCount` is a count and `CreationTime` a point in time, so both are
/// ordered as NUMBERS — a count of 10 after one of 9, and an earlier instant
/// before a later one whatever subsecond digits either timestamp was rendered
/// with. Every other sortable member is a string and is ordered as text. One
/// query sorts on one member, so every value compared in it is built by the same
/// arm.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryLogConfigSortValue {
    Number(i128),
    Text(String),
}

/// A stored `Rfc3339TimeString` as nanoseconds since the epoch, so two
/// timestamps are ordered by the instants they name rather than by their text.
///
/// A string this service did not render (there is none: every stored timestamp
/// comes from [`now_rfc3339`]) orders as the epoch.
fn rfc3339_epoch_nanos(raw: &str) -> i128 {
    aws_smithy_types::DateTime::from_str(raw, aws_smithy_types::date_time::Format::DateTime)
        .map(|at| i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos()))
        .unwrap_or_default()
}

/// A stored `Rfc3339TimeString` as the whole Unix seconds it names, the spelling
/// `Filter$Name` documents for a `CreationTime` filter's values. Rendered from
/// the same parse as [`rfc3339_epoch_nanos`], so the two readings of one stored
/// timestamp cannot disagree.
fn rfc3339_unix_seconds(raw: &str) -> String {
    (rfc3339_epoch_nanos(raw) / 1_000_000_000).to_string()
}

/// Mint the `Id` of a query logging configuration from the id sequence. AWS names
/// one `rqlc-<hex>`. Every result fits `ResourceId` (length 1..64).
pub fn mint_query_log_config_id(seq: u64) -> String {
    format!("rqlc-{:016x}", scramble(seq))
}

/// Mint the `Id` of one VPC-to-configuration association from the id sequence.
/// AWS names one `rqlca-<hex>`, the association form of the configuration's own
/// `rqlc-` prefix. Every result fits `ResourceId` (length 1..64).
pub fn mint_query_log_config_association_id(seq: u64) -> String {
    format!("rqlca-{:016x}", scramble(seq))
}

/// The most tags one resource carries.
///
/// The model's `TagList` is length 0..200 — it is both the list `TagResource`
/// takes and the list `ListTagsForResource` reports — so 200 tags is the most
/// one call can hand over and the most a resource can report back. A write that
/// would take a resource past it is refused with the `LimitExceededException`
/// `TagResource` models, and the stored set is left exactly as it was.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The resource-type segment of each taggable resource's ARN, with the modelled
/// shape name of that resource.
///
/// The segments are the ones the create operations mint an ARN out of
/// (`resolver-endpoint/<id>`, `resolver-rule/<id>`, `outpost-resolver/<id>`,
/// `firewall-domain-list/<id>`, `resolver-query-log-config/<id>`); the shape
/// names are what every other refusal of this service reports in
/// `ResourceNotFoundException$ResourceType`.
const TAGGABLE_ARN_SEGMENTS: [(&str, &str); 6] = [
    ("resolver-endpoint", RESOURCE_TYPE_RESOLVER_ENDPOINT),
    ("resolver-rule", RESOURCE_TYPE_RESOLVER_RULE),
    ("outpost-resolver", RESOURCE_TYPE_OUTPOST_RESOLVER),
    ("firewall-domain-list", RESOURCE_TYPE_FIREWALL_DOMAIN_LIST),
    ("firewall-rule-group", RESOURCE_TYPE_FIREWALL_RULE_GROUP),
    (
        "resolver-query-log-config",
        RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG,
    ),
];

/// The resource type an ARN states, when the ARN names one of this service's
/// taggable resources: the segment that follows the account id, up to the `/`
/// that separates it from the resource id
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`).
///
/// `None` for anything else, so a tag operation's refusal names a resource type
/// only when the ARN it was handed actually states one.
pub fn arn_resource_type(resource_arn: &str) -> Option<&'static str> {
    let (segment, _) = resource_arn.split(':').nth(5)?.split_once('/')?;
    TAGGABLE_ARN_SEGMENTS
        .iter()
        .find(|(name, _)| *name == segment)
        .map(|(_, resource_type)| *resource_type)
}

/// A resource of this service that carries tags, as a tag operation's
/// `ResourceArn` resolves it: which collection holds it, and its key there.
///
/// `TagResource$ResourceArn`'s doc names the Get and List commands an ARN comes
/// from, so the resources whose records carry an `Arn` are the ones the tag
/// operations address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    ResolverEndpoint(String),
    ResolverRule(String),
    OutpostResolver(String),
    FirewallDomainList(String),
    FirewallRuleGroup(String),
    ResolverQueryLogConfig(String),
}

/// The id an ARN names the resource by: everything after the last `/`, the
/// separator between an ARN's resource type and its resource id
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`).
///
/// A string carrying no `/` is taken as naming the resource by itself, so the
/// refusal still states what was asked for.
pub fn arn_resource_id(resource_arn: &str) -> &str {
    match resource_arn.rsplit_once('/') {
        Some((_, id)) => id,
        None => resource_arn,
    }
}

/// The `ResourceNotFoundException` the tag operations answer for an ARN this
/// store holds no resource under.
///
/// Built here so `TagResource`, `UntagResource` and `ListTagsForResource`
/// cannot drift apart: all of them report the RSLVR-coded message, which names
/// the resource by the id its ARN carries, and the resource type the model asks
/// for.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(arn_resource_id(resource_arn))),
        resource_type: arn_resource_type(resource_arn).map(str::to_string),
    }
}

/// The `LimitExceededException` `TagResource` answers for a write that would
/// take a resource past [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// The sentence states the limit the model's `TagList` length fixes and nothing
/// from the request, so the rejected tags are not echoed back. No recorded
/// response available here states the wording Route 53 Resolver uses for this
/// fault, and the model states no message text, so — as for
/// [`outpost_resolver_not_found`] — the HTTP status (400) and the error shape
/// are what the contract fixes and the wording is an example.
pub fn tag_limit_exceeded(resource_arn: &str) -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "A resource can have at most {MAX_TAGS_PER_RESOURCE} tags"
        )),
        resource_type: arn_resource_type(resource_arn).map(str::to_string),
    }
}

/// Add `requested` to `stored`, one resource's tag set.
///
/// A tag write is an upsert keyed on the tag key: a key the resource already
/// carries keeps its position and takes the requested value, and a key new to
/// the resource is appended — so the order a resource reports its tags in is the
/// order its keys were first written in, and no key is ever reported twice.
/// Keys are compared exactly, as tag keys are case-sensitive. `requested` is
/// applied in order, so a request that names one key twice leaves the value it
/// named last.
///
/// Answers `false`, leaving `stored` EXACTLY as it was, when the result would
/// hold more than [`MAX_TAGS_PER_RESOURCE`] tags.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) -> bool {
    let projected = {
        let mut keys: HashSet<&str> = stored.iter().map(|tag| tag.key.as_str()).collect();
        keys.extend(requested.iter().map(|tag| tag.key.as_str()));
        keys.len()
    };
    if projected > MAX_TAGS_PER_RESOURCE {
        return false;
    }

    for tag in requested {
        match stored.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => stored.push(tag.clone()),
        }
    }
    true
}

/// Drop every tag of `stored` whose key is named in `keys`, and leave the rest
/// in the order they were written in.
///
/// Idempotent: a key the resource does not carry removes nothing, so removing
/// the same keys twice leaves the same tag set.
pub fn remove_tags(stored: &mut Vec<Tag>, keys: &[String]) {
    stored.retain(|tag| !keys.contains(&tag.key));
}

/// The cursor scope of one `ListTagsForResource` query.
///
/// The offset a cursor carries is an index into ONE resource's tags, so — for
/// the same reason as [`endpoint_list_scope`] — a cursor minted while paging one
/// resource is not read as an offset into another resource's tags, but refused
/// as a cursor this query did not issue.
pub fn tag_list_scope(resource_arn: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListTagsForResource/resource-arn/v1");
    feed_field(&mut hasher, resource_arn.as_bytes());
    format!("ListTagsForResource/{}", hex::encode(&hasher.finalize()[..8]))
}

/// Per-service state for com-amazonaws-route53resolver-route53resolver. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53resolverRoute53resolverState {
    /// Every stored Resolver endpoint, keyed by its `Id`.
    pub resolver_endpoints: HashMap<String, ResolverEndpointRecord>,
    /// `CreatorRequestId` -> endpoint id, so a retried create with the same
    /// `CreatorRequestId` resolves to the endpoint it already made instead of a
    /// duplicate.
    pub creator_request_id_index: HashMap<String, String>,
    /// Every stored Resolver on an Outpost, keyed by its `Id`.
    pub outpost_resolvers: HashMap<String, OutpostResolverRecord>,
    /// `CreatorRequestId` -> Resolver-on-Outpost id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resource. Kept separate
    /// because a `CreatorRequestId` identifies a request to ONE operation: the
    /// same string used for an endpoint and for a Resolver on an Outpost names
    /// two different resources.
    pub outpost_resolver_creator_request_ids: HashMap<String, String>,
    /// Every stored Resolver rule, keyed by its `Id`.
    pub resolver_rules: HashMap<String, ResolverRuleRecord>,
    /// `CreatorRequestId` -> rule id, so a retried create with the same
    /// `CreatorRequestId` resolves to the rule it already made instead of a
    /// duplicate.
    pub rule_creator_request_id_index: HashMap<String, String>,
    /// Every stored rule-to-VPC association, keyed by its `Id`.
    pub resolver_rule_associations: HashMap<String, ResolverRuleAssociationRecord>,
    /// Every stored firewall domain list, keyed by its `Id`.
    pub firewall_domain_lists: HashMap<String, FirewallDomainListRecord>,
    /// `CreatorRequestId` -> firewall domain list id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub firewall_domain_list_creator_request_ids: HashMap<String, String>,
    /// Every stored DNS Firewall rule group, keyed by its `Id`.
    pub firewall_rule_groups: HashMap<String, FirewallRuleGroupRecord>,
    /// `CreatorRequestId` -> firewall rule group id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub firewall_rule_group_creator_request_ids: HashMap<String, String>,
    /// Every stored Resolver query logging configuration, keyed by its `Id`.
    pub resolver_query_log_configs: HashMap<String, ResolverQueryLogConfigRecord>,
    /// `CreatorRequestId` -> query logging configuration id, the same retry
    /// index as [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub query_log_config_creator_request_ids: HashMap<String, String>,
    /// The configuration for DNSSEC validation of every VPC whose validation
    /// setting was moved off its default, keyed by that VPC's id
    /// (`ResolverDnssecConfig$ResourceId`).
    ///
    /// A configuration for DNSSEC validation is a per-VPC singleton the service
    /// maintains itself, so this map is an override table and not an inventory:
    /// a VPC it holds no entry for still HAS a configuration, the default one
    /// [`Self::resolver_dnssec_config`] derives.
    pub resolver_dnssec_configs: HashMap<String, ResolverDnssecConfigRecord>,
    /// The Resolver behavior configuration of every VPC whose configuration was
    /// moved off its default, keyed by that VPC's id
    /// (`ResolverConfig$ResourceId`).
    ///
    /// A Resolver configuration is a per-VPC singleton the service maintains
    /// itself, so this map is an override table and not an inventory: a VPC it
    /// holds no entry for still HAS a configuration, the default one
    /// [`Self::resolver_config`] derives.
    pub resolver_configs: HashMap<String, ResolverConfigRecord>,
    /// The DNS Firewall configuration of every VPC whose configuration was
    /// moved off its default, keyed by that VPC's id (`FirewallConfig$ResourceId`).
    ///
    /// A firewall configuration is a per-VPC singleton the service maintains
    /// itself, so this map is an override table and not an inventory: a VPC it
    /// holds no entry for still HAS a configuration, the default one
    /// [`Self::firewall_config`] derives.
    pub firewall_configs: HashMap<String, FirewallConfigRecord>,
    /// Monotonic counter behind every minted id (endpoint, IP, Resolver on an
    /// Outpost, rule, association, DNSSEC configuration), so ids are
    /// reproducible within a run and never collide — across resources as well
    /// as within one.
    pub id_seq: u64,
}

impl ComAmazonawsRoute53resolverRoute53resolverState {
    /// Whether one more endpoint may be stored, i.e. the store is still below
    /// [`MAX_STORED_ENDPOINTS`]. A create that this refuses answers
    /// `LimitExceededException` and writes nothing.
    pub fn can_store_another_endpoint(&self) -> bool {
        self.resolver_endpoints.len() < MAX_STORED_ENDPOINTS
    }

    /// Take the next value of the id sequence.
    pub fn next_seq(&mut self) -> u64 {
        self.id_seq = self.id_seq.saturating_add(1);
        self.id_seq
    }

    /// Whether one more Resolver rule may be stored, i.e. the store is still
    /// below [`MAX_STORED_RULES`].
    pub fn can_store_another_rule(&self) -> bool {
        self.resolver_rules.len() < MAX_STORED_RULES
    }

    /// Whether one more rule association may be stored, i.e. the store is still
    /// below [`MAX_STORED_RULE_ASSOCIATIONS`].
    pub fn can_store_another_rule_association(&self) -> bool {
        self.resolver_rule_associations.len() < MAX_STORED_RULE_ASSOCIATIONS
    }

    /// The association between `resolver_rule_id` and `vpc_id`, if the store
    /// holds one.
    ///
    /// A rule and a VPC are associated at most once, so this is what
    /// `AssociateResolverRule` consults before writing a second record for the
    /// same pair. The scan is over a store bounded by
    /// [`MAX_STORED_RULE_ASSOCIATIONS`].
    pub fn find_rule_association(
        &self,
        resolver_rule_id: &str,
        vpc_id: &str,
    ) -> Option<&ResolverRuleAssociationRecord> {
        self.resolver_rule_associations
            .values()
            .find(|record| record.resolver_rule_id == resolver_rule_id && record.vpc_id == vpc_id)
    }

    /// The stored Resolver rule `arn` names, if the store holds one.
    ///
    /// The rule policy operations address a rule by its `Arn` and not by its
    /// `ResolverRuleId`, so they resolve it through this. The whole ARN is
    /// compared, so an ARN naming another account, another region or another
    /// resource type matches no rule. The scan is over a store bounded by
    /// [`MAX_STORED_RULES`].
    pub fn rule_by_arn(&self, arn: &str) -> Option<&ResolverRuleRecord> {
        self.resolver_rules.values().find(|record| record.arn == arn)
    }

    /// The same lookup as [`Self::rule_by_arn`], for the write side.
    pub fn rule_by_arn_mut(&mut self, arn: &str) -> Option<&mut ResolverRuleRecord> {
        self.resolver_rules
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// How many VPCs a rule is still associated with.
    ///
    /// `DeleteResolverRule` is what reads it: "Before you can delete a Resolver
    /// rule, you must disassociate it from all the VPCs that you associated the
    /// Resolver rule with", so a rule with any association left cannot be
    /// deleted. The scan is over a store bounded by
    /// [`MAX_STORED_RULE_ASSOCIATIONS`].
    pub fn rule_association_count(&self, resolver_rule_id: &str) -> usize {
        self.resolver_rule_associations
            .values()
            .filter(|record| record.resolver_rule_id == resolver_rule_id)
            .count()
    }

    /// Every stored endpoint in creation order — the order both the list
    /// operation's pages and its cursor are defined over.
    pub fn endpoints_in_creation_order(&self) -> Vec<&ResolverEndpointRecord> {
        let mut all: Vec<&ResolverEndpointRecord> = self.resolver_endpoints.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored Resolver rule in creation order — the order
    /// `ListResolverRules`' pages and its cursor are defined over, for the same
    /// reason as [`Self::endpoints_in_creation_order`]: a page is cut at an
    /// offset into this sequence, so the sequence must not depend on the hash
    /// order of the map that holds the records.
    pub fn rules_in_creation_order(&self) -> Vec<&ResolverRuleRecord> {
        let mut all: Vec<&ResolverRuleRecord> = self.resolver_rules.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored rule association in creation order — the order
    /// `ListResolverRuleAssociations`' pages and its cursor are defined over,
    /// for the same reason as [`Self::rules_in_creation_order`].
    pub fn rule_associations_in_creation_order(&self) -> Vec<&ResolverRuleAssociationRecord> {
        let mut all: Vec<&ResolverRuleAssociationRecord> =
            self.resolver_rule_associations.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Whether one more Resolver on an Outpost may be stored, i.e. the store is
    /// still below [`MAX_STORED_OUTPOST_RESOLVERS`]. A create that this refuses
    /// answers `ServiceQuotaExceededException` and writes nothing.
    pub fn can_store_another_outpost_resolver(&self) -> bool {
        self.outpost_resolvers.len() < MAX_STORED_OUTPOST_RESOLVERS
    }

    /// Whether one more firewall domain list may be stored, i.e. the store is
    /// still below [`MAX_STORED_FIREWALL_DOMAIN_LISTS`]. A create that this
    /// refuses answers `LimitExceededException` and writes nothing.
    pub fn can_store_another_firewall_domain_list(&self) -> bool {
        self.firewall_domain_lists.len() < MAX_STORED_FIREWALL_DOMAIN_LISTS
    }

    /// Whether one more firewall rule group may be stored, i.e. the store is
    /// still below [`MAX_STORED_FIREWALL_RULE_GROUPS`]. A create that this
    /// refuses answers `LimitExceededException` and writes nothing.
    pub fn can_store_another_firewall_rule_group(&self) -> bool {
        self.firewall_rule_groups.len() < MAX_STORED_FIREWALL_RULE_GROUPS
    }

    /// Whether one more query logging configuration may be stored, i.e. the
    /// store is still below [`MAX_STORED_QUERY_LOG_CONFIGS`]. A create that this
    /// refuses answers `LimitExceededException` and writes nothing.
    pub fn can_store_another_query_log_config(&self) -> bool {
        self.resolver_query_log_configs.len() < MAX_STORED_QUERY_LOG_CONFIGS
    }

    /// The VPC-to-configuration association the store holds under
    /// `association_id`, if any.
    ///
    /// An association lives on the configuration it belongs to, so resolving one
    /// by its own id walks the configurations; both collections are bounded
    /// ([`MAX_STORED_QUERY_LOG_CONFIGS`],
    /// [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`]).
    pub fn query_log_config_association(
        &self,
        association_id: &str,
    ) -> Option<&ResolverQueryLogConfigAssociationRecord> {
        self.resolver_query_log_configs
            .values()
            .flat_map(|config| config.associations.iter())
            .find(|record| record.id == association_id)
    }

    /// Every stored query logging configuration in creation order — the order
    /// `ListResolverQueryLogConfigs`' unsorted pages and its cursor are defined
    /// over, for the same reason as [`Self::endpoints_in_creation_order`]: a
    /// page is cut at an offset into this sequence, so the sequence must not
    /// depend on the hash order of the map that holds the records.
    pub fn query_log_configs_in_creation_order(&self) -> Vec<&ResolverQueryLogConfigRecord> {
        let mut all: Vec<&ResolverQueryLogConfigRecord> =
            self.resolver_query_log_configs.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored Resolver on an Outpost in creation order — the order
    /// `ListOutpostResolvers`' pages and its cursor are defined over.
    pub fn outpost_resolvers_in_creation_order(&self) -> Vec<&OutpostResolverRecord> {
        let mut all: Vec<&OutpostResolverRecord> = self.outpost_resolvers.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Whether one more configuration for DNSSEC validation may be stored,
    /// i.e. the store is still below [`MAX_STORED_DNSSEC_CONFIGS`]. An update
    /// that this refuses writes nothing.
    pub fn can_store_another_dnssec_config(&self) -> bool {
        self.resolver_dnssec_configs.len() < MAX_STORED_DNSSEC_CONFIGS
    }

    /// The configuration for DNSSEC validation of the VPC `resource_id`, which
    /// every VPC has: the stored record when one was written for it, and
    /// otherwise the default configuration that VPC reports — identical
    /// identity ([`dnssec_config_id`] of the VPC, owned by this account) with
    /// validation off, settled.
    ///
    /// A pure read: a VPC with no record is NOT given one here, so reporting a
    /// configuration never changes what a later caller sees.
    pub fn resolver_dnssec_config(&self, resource_id: &str) -> ResolverDnssecConfigRecord {
        self.resolver_dnssec_configs
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| ResolverDnssecConfigRecord {
                seq: 0,
                id: dnssec_config_id(resource_id),
                owner_id: ::engine::emulator_account_id().to_string(),
                resource_id: resource_id.to_string(),
                status: DNSSEC_STATUS_DISABLED.to_string(),
                settled_status: DNSSEC_STATUS_DISABLED.to_string(),
                settled_at: Instant::now(),
            })
    }

    /// Every stored configuration for DNSSEC validation in creation order — the
    /// order `ListResolverDnssecConfigs`' pages and its cursor are defined over,
    /// for the same reason as [`Self::endpoints_in_creation_order`]: a page is
    /// cut at an offset into this sequence, so the sequence must not depend on
    /// the hash order of the map that holds the records.
    pub fn dnssec_configs_in_creation_order(&self) -> Vec<&ResolverDnssecConfigRecord> {
        let mut all: Vec<&ResolverDnssecConfigRecord> =
            self.resolver_dnssec_configs.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// The DNS Firewall configuration of the VPC `resource_id`, which every VPC
    /// has: the stored record when one was written for it, and otherwise the
    /// default configuration that VPC reports — identical identity
    /// ([`firewall_config_id`] of the VPC, owned by this account) with
    /// `FirewallFailOpen` at its documented default.
    ///
    /// A pure read: a VPC with no record is NOT given one here, so reporting a
    /// configuration never changes what a later caller sees.
    pub fn firewall_config(&self, resource_id: &str) -> FirewallConfigRecord {
        self.firewall_configs
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| FirewallConfigRecord {
                seq: 0,
                id: firewall_config_id(resource_id),
                resource_id: resource_id.to_string(),
                owner_id: ::engine::emulator_account_id().to_string(),
                firewall_fail_open: FIREWALL_FAIL_OPEN_DISABLED.to_string(),
            })
    }

    /// Whether one more VPC's firewall configuration may be stored, i.e. the
    /// store is still below [`MAX_STORED_FIREWALL_CONFIGS`]. Asked only for a
    /// VPC [`Self::firewall_configs`] holds no record for yet.
    pub fn can_store_another_firewall_config(&self) -> bool {
        self.firewall_configs.len() < MAX_STORED_FIREWALL_CONFIGS
    }

    /// Every VPC this store knows the account has, in the order the service came
    /// to know of each one.
    ///
    /// A per-VPC singleton configuration ([`FirewallConfigRecord`],
    /// [`ResolverConfigRecord`]) exists for every VPC of the account, not only
    /// for those a caller moved off the default, so the operation that LISTS
    /// such configurations enumerates VPCs and not override records. This
    /// emulator keeps no VPC inventory of its own, so the VPCs it knows the
    /// account has are exactly those its own records name: a VPC a Resolver rule
    /// is associated with, a VPC a Resolver endpoint is hosted in, and a VPC one
    /// of the per-VPC configurations was written for.
    ///
    /// Ordered by the position in the store's creation order ([`Self::id_seq`])
    /// of the FIRST record that named the VPC, so the sequence never depends on
    /// the hash order of the maps holding those records and a VPC that becomes
    /// known later only ever appends — which is what lets a list operation cut a
    /// page at an offset into it. A pure read: a VPC is not given any record
    /// here.
    pub fn known_vpc_ids(&self) -> Vec<String> {
        let named = self
            .resolver_rule_associations
            .values()
            .map(|record| (record.vpc_id.as_str(), record.seq))
            .chain(self.resolver_endpoints.values().filter_map(|record| {
                record
                    .host_vpc_id
                    .as_deref()
                    .map(|vpc_id| (vpc_id, record.seq))
            }))
            .chain(
                self.resolver_dnssec_configs
                    .values()
                    .map(|record| (record.resource_id.as_str(), record.seq)),
            )
            .chain(
                self.resolver_configs
                    .values()
                    .map(|record| (record.resource_id.as_str(), record.seq)),
            )
            .chain(
                self.firewall_configs
                    .values()
                    .map(|record| (record.resource_id.as_str(), record.seq)),
            );

        let mut first_seen: HashMap<&str, u64> = HashMap::new();
        for (vpc_id, seq) in named {
            let earliest = first_seen.entry(vpc_id).or_insert(seq);
            *earliest = (*earliest).min(seq);
        }

        // The id and the sequence number together, so VPCs that became known in
        // the same call still have ONE order rather than the map's.
        let mut ordered: Vec<(u64, &str)> = first_seen
            .into_iter()
            .map(|(vpc_id, seq)| (seq, vpc_id))
            .collect();
        ordered.sort_unstable();
        ordered
            .into_iter()
            .map(|(_, vpc_id)| vpc_id.to_string())
            .collect()
    }

    /// The Resolver behavior configuration of the VPC `resource_id`, which every
    /// VPC has: the stored record when one was written for it, and otherwise the
    /// default configuration that VPC reports — identical identity
    /// ([`resolver_config_id`] of the VPC, owned by this account) with
    /// `AutodefinedReverse` at its documented default, settled.
    ///
    /// A pure read: a VPC with no record is NOT given one here, so reporting a
    /// configuration never changes what a later caller sees.
    pub fn resolver_config(&self, resource_id: &str) -> ResolverConfigRecord {
        self.resolver_configs
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| ResolverConfigRecord {
                seq: 0,
                id: resolver_config_id(resource_id),
                resource_id: resource_id.to_string(),
                owner_id: ::engine::emulator_account_id().to_string(),
                status: AUTODEFINED_REVERSE_ENABLED.to_string(),
                settled_status: AUTODEFINED_REVERSE_ENABLED.to_string(),
                settled_at: Instant::now(),
            })
    }

    /// Whether one more VPC's Resolver configuration may be stored, i.e. the
    /// store is still below [`MAX_STORED_RESOLVER_CONFIGS`]. Asked only for a
    /// VPC [`Self::resolver_configs`] holds no record for yet.
    pub fn can_store_another_resolver_config(&self) -> bool {
        self.resolver_configs.len() < MAX_STORED_RESOLVER_CONFIGS
    }

    /// Every stored firewall domain list in creation order — the order
    /// `ListFirewallDomainLists`' pages and its cursor are defined over, for the
    /// same reason as [`Self::endpoints_in_creation_order`]: a page is cut at an
    /// offset into this sequence, so the sequence must not depend on the hash
    /// order of the map that holds the records.
    pub fn firewall_domain_lists_in_creation_order(&self) -> Vec<&FirewallDomainListRecord> {
        let mut all: Vec<&FirewallDomainListRecord> = self.firewall_domain_lists.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored firewall rule group in creation order — the order
    /// `ListFirewallRuleGroups`' pages and its cursor are defined over, for the
    /// same reason as [`Self::firewall_domain_lists_in_creation_order`]: a page
    /// is cut at an offset into this sequence, so the sequence must not depend
    /// on the hash order of the map that holds the records.
    pub fn firewall_rule_groups_in_creation_order(&self) -> Vec<&FirewallRuleGroupRecord> {
        let mut all: Vec<&FirewallRuleGroupRecord> = self.firewall_rule_groups.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// The resource an ARN names, when this store holds one under it.
    ///
    /// Matched against the ARN each record was minted with, so an ARN resolves
    /// only to the resource it was issued for — in the region and account that
    /// created it — and no ARN text is parsed to find the record. Each scan is
    /// over a store bounded by the `MAX_STORED_*` cap of its resource.
    pub fn find_tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        if let Some(record) = self
            .resolver_endpoints
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::ResolverEndpoint(record.id.clone()));
        }
        if let Some(record) = self
            .resolver_rules
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::ResolverRule(record.id.clone()));
        }
        if let Some(record) = self
            .outpost_resolvers
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::OutpostResolver(record.id.clone()));
        }
        if let Some(record) = self
            .firewall_domain_lists
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::FirewallDomainList(record.id.clone()));
        }
        if let Some(record) = self
            .firewall_rule_groups
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::FirewallRuleGroup(record.id.clone()));
        }
        if let Some(record) = self
            .resolver_query_log_configs
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::ResolverQueryLogConfig(record.id.clone()));
        }
        None
    }

    /// The tags of the resource `resource_arn` names, for reading, or `None`
    /// when this store holds no resource under that ARN.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&[Tag]> {
        match self.find_tagged_resource(resource_arn)? {
            TaggedResource::ResolverEndpoint(id) => self
                .resolver_endpoints
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::ResolverRule(id) => self
                .resolver_rules
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::OutpostResolver(id) => self
                .outpost_resolvers
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::FirewallDomainList(id) => self
                .firewall_domain_lists
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::FirewallRuleGroup(id) => self
                .firewall_rule_groups
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::ResolverQueryLogConfig(id) => self
                .resolver_query_log_configs
                .get(&id)
                .map(|record| record.tags.as_slice()),
        }
    }

    /// The same tag set, for writing: the tags live ON the resource's record, so
    /// a tag write is a write to that record and every operation that reports
    /// the resource sees it.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        match self.find_tagged_resource(resource_arn)? {
            TaggedResource::ResolverEndpoint(id) => self
                .resolver_endpoints
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::ResolverRule(id) => self
                .resolver_rules
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::OutpostResolver(id) => self
                .outpost_resolvers
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::FirewallDomainList(id) => self
                .firewall_domain_lists
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::FirewallRuleGroup(id) => self
                .firewall_rule_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::ResolverQueryLogConfig(id) => self
                .resolver_query_log_configs
                .get_mut(&id)
                .map(|record| &mut record.tags),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53resolverRoute53resolverState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53resolverRoute53resolverState>)
}

/// The wall clock, as the `Rfc3339TimeString` the model's time members declare
/// (length 20..40).
pub fn now_rfc3339() -> String {
    aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The region the request was signed for, which is the region of every ARN the
/// request mints. Falls back to the emulator's default region when the request
/// carries no SigV4 credential scope.
pub fn request_region(headers: &http::HeaderMap) -> String {
    emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Spread a counter value over 64 bits, so a minted id reads like the opaque hex
/// AWS returns while staying a pure function of the counter. splitmix64's
/// finaliser is a bijection on `u64`, so distinct counter values never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Mint the `Id` of a Resolver endpoint from the id sequence.
///
/// AWS names an inbound endpoint `rslvr-in-<hex>` and an outbound one
/// `rslvr-out-<hex>`; `INBOUND_DELEGATION` is an inbound direction, so it takes
/// the inbound infix. Every result fits `ResourceId` (length 1..64).
pub fn mint_endpoint_id(direction: &str, seq: u64) -> String {
    let infix = if direction == ENDPOINT_DIRECTION_OUTBOUND {
        "out"
    } else {
        "in"
    };
    format!("rslvr-{infix}-{:016x}", scramble(seq))
}

/// Mint the `IpId` of one endpoint IP address from the id sequence. AWS names a
/// Resolver network interface `rni-<hex>`.
pub fn mint_ip_id(seq: u64) -> String {
    format!("rni-{:016x}", scramble(seq))
}

/// Mint the `Id` of a Resolver on an Outpost from the id sequence.
///
/// The model constrains `ResourceId` only to length 1..64, so the shape is
/// taken from [`mint_endpoint_id`]'s `rslvr-<infix>-<hex>` with the Outpost
/// infix: an id is therefore recognisable as this resource's and fits
/// `ResourceId`.
pub fn mint_outpost_resolver_id(seq: u64) -> String {
    format!("rslvr-op-{:016x}", scramble(seq))
}

/// Mint the `Id` of a Resolver rule from the id sequence. AWS names a Resolver
/// rule `rslvr-rr-<hex>`. Every result fits `ResourceId` (length 1..64).
pub fn mint_rule_id(seq: u64) -> String {
    format!("rslvr-rr-{:016x}", scramble(seq))
}

/// Mint the `Id` of a rule-to-VPC association from the id sequence. AWS names a
/// Resolver rule association `rslvr-rrassoc-<hex>`.
pub fn mint_rule_association_id(seq: u64) -> String {
    format!("rslvr-rrassoc-{:016x}", scramble(seq))
}

/// Mint the `Id` of a firewall domain list from the id sequence. AWS names a
/// DNS Firewall domain list `rslvr-fdl-<hex>`. Every result fits `ResourceId`
/// (length 1..64).
pub fn mint_firewall_domain_list_id(seq: u64) -> String {
    format!("rslvr-fdl-{:016x}", scramble(seq))
}

/// Mint the `Id` of a firewall rule group from the id sequence. AWS names a DNS
/// Firewall rule group `rslvr-frg-<hex>`. Every result fits `ResourceId`
/// (length 1..64).
pub fn mint_firewall_rule_group_id(seq: u64) -> String {
    format!("rslvr-frg-{:016x}", scramble(seq))
}

/// The `Id` of a VPC's DNS Firewall configuration. AWS names one
/// `rslvr-fc-<hex>`; every result fits `ResourceId` (length 1..64).
///
/// Derived from the VPC id rather than taken from the id sequence, because no
/// operation creates a firewall configuration: one exists for a VPC whether or
/// not anything has ever written to it, so its id has to be a function of the
/// VPC it belongs to. The same VPC therefore reports the same `Id` from every
/// call, before and after an update.
pub fn firewall_config_id(resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/firewall-config/v1");
    feed_field(&mut hasher, resource_id.as_bytes());
    format!("rslvr-fc-{}", hex::encode(&hasher.finalize()[..8]))
}

/// The `Id` of a VPC's Resolver behavior configuration. AWS names one
/// `rslvr-rc-<hex>`; every result fits `ResourceId` (length 1..64).
///
/// Derived from the VPC id rather than taken from the id sequence, for the same
/// reason as [`firewall_config_id`]: no operation creates a Resolver
/// configuration, so one exists for a VPC whether or not anything has ever
/// written to it, and its id has to be a function of the VPC it belongs to. The
/// same VPC therefore reports the same `Id` from every call, before and after an
/// update.
pub fn resolver_config_id(resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/resolver-config/v1");
    feed_field(&mut hasher, resource_id.as_bytes());
    format!("rslvr-rc-{}", hex::encode(&hasher.finalize()[..8]))
}

/// The `HostVPCId` of an endpoint placed in `subnet_ids`.
///
/// `Filter`'s doc for `HostVPCId` says "In a CreateResolverEndpoint request,
/// SubnetId indirectly identifies the VPC", and the emulator holds no VPC
/// inventory to look the subnets up in, so the id is derived from the subnets
/// themselves: the same set of subnets always yields the same VPC id, and a
/// second endpoint in those subnets reports the same `HostVPCId` as the first.
pub fn host_vpc_id(subnet_ids: &[String]) -> String {
    let mut distinct: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
    distinct.sort_unstable();
    distinct.dedup();
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/host-vpc/v1");
    for subnet in distinct {
        hasher.update(b":");
        hasher.update(subnet.as_bytes());
    }
    let digest = hasher.finalize();
    format!("vpc-{}", hex::encode(&digest[..8]))
}

/// Feed a length-prefixed field into a digest, so two different sequences of
/// fields cannot hash to the same byte stream whatever the fields contain.
fn feed_field(hasher: &mut Sha256, field: &[u8]) {
    hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(field);
}

/// The cursor scope of one `ListResolverEndpoints` query.
///
/// The offset a cursor carries is an index into the FILTERED collection, so a
/// cursor only means anything for the filter set it was minted under. Folding
/// that set into the scope (canonically, so the order the filters and their
/// values arrive in does not matter) means resuming with a different `Filters`
/// is answered with `InvalidNextTokenException` rather than with a page cut at
/// the wrong position.
pub fn endpoint_list_scope(filters: &[(FilterKey, HashSet<String>)]) -> String {
    filter_list_scope(
        "ListResolverEndpoints",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// The cursor scope of one `ListResolverRules` query, for the same reason and in
/// the same shape as [`endpoint_list_scope`]'s.
pub fn rule_list_scope(filters: &[(RuleFilterKey, HashSet<String>)]) -> String {
    filter_list_scope(
        "ListResolverRules",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// The cursor scope of one `ListResolverRuleAssociations` query, for the same
/// reason and in the same shape as [`endpoint_list_scope`]'s.
pub fn rule_association_list_scope(
    filters: &[(RuleAssociationFilterKey, HashSet<String>)],
) -> String {
    filter_list_scope(
        "ListResolverRuleAssociations",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// The cursor scope of one filtered list query: the operation being paged, plus
/// a digest of its filter set folded canonically, so neither the order the
/// filters arrived in nor the order of one filter's values changes the scope.
fn filter_list_scope<'a>(
    operation: &str,
    filters: impl Iterator<Item = (&'static str, &'a HashSet<String>)>,
) -> String {
    let mut described: Vec<(&'static str, Vec<&str>)> = filters
        .map(|(name, values)| {
            let mut values: Vec<&str> = values.iter().map(String::as_str).collect();
            values.sort_unstable();
            (name, values)
        })
        .collect();
    described.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/list-filters/v1");
    feed_field(&mut hasher, operation.as_bytes());
    for (name, values) in &described {
        feed_field(&mut hasher, name.as_bytes());
        for value in values {
            feed_field(&mut hasher, value.as_bytes());
        }
    }
    format!("{operation}/{}", hex::encode(&hasher.finalize()[..8]))
}

/// The cursor scope of one `ListResolverQueryLogConfigs` query.
///
/// The offset a cursor carries is an index into the filtered, sorted collection,
/// so — as for [`endpoint_list_scope`] — the filter set belongs in the scope;
/// the ordering does too, because "if you submit a second or subsequent
/// ListResolverQueryLogConfigs request and specify the NextToken parameter, you
/// must use the same value for SortBy / SortOrder, if any, as in the previous
/// request": a resume that changes either is answered as a cursor this query
/// does not accept, rather than with a page cut at the wrong position.
pub fn query_log_config_list_scope(
    filters: &[(QueryLogConfigFilterKey, HashSet<String>)],
    sort_by: Option<QueryLogConfigSortKey>,
    sort_order: Option<&str>,
) -> String {
    let base = filter_list_scope(
        "ListResolverQueryLogConfigs",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    );
    let sort_by = sort_by.map_or("", QueryLogConfigSortKey::canonical_name);
    format!("{base}/{sort_by}/{}", sort_order.unwrap_or_default())
}

/// The cursor scope of one `ListOutpostResolvers` query.
///
/// The offset a cursor carries is an index into the collection the query
/// selects, which `OutpostArn` narrows, so — exactly as for
/// [`endpoint_list_scope`] — resuming with a different `OutpostArn` (or with
/// none) is answered as an unknown cursor rather than with a page cut at the
/// wrong position.
pub fn outpost_resolver_list_scope(outpost_arn: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListOutpostResolvers/outpost-arn/v1");
    if let Some(outpost_arn) = outpost_arn {
        feed_field(&mut hasher, outpost_arn.as_bytes());
    }
    format!(
        "ListOutpostResolvers/{}",
        hex::encode(&hasher.finalize()[..8])
    )
}

/// The cursor scope of one `ListFirewallDomainLists` query.
///
/// `ListFirewallDomainLists` models no filter at all — only `MaxResults` and
/// `NextToken` — so every query of it pages ONE collection and the scope is a
/// constant. It still names the operation, so a cursor minted by another list
/// operation is not read as an offset into this one (see
/// [`endpoint_list_scope`]).
pub const FIREWALL_DOMAIN_LIST_LIST_SCOPE: &str = "ListFirewallDomainLists/all/v1";

/// The cursor scope of one `ListFirewallConfigs` query.
///
/// `ListFirewallConfigs` models no filter either, so — for the same reason as
/// [`FIREWALL_DOMAIN_LIST_LIST_SCOPE`] — the scope is a constant that names the
/// operation.
pub const FIREWALL_CONFIG_LIST_SCOPE: &str = "ListFirewallConfigs/all/v1";

/// The cursor scope of one `ListFirewallRuleGroups` query.
///
/// `ListFirewallRuleGroups` models no filter either — only `MaxResults` and
/// `NextToken` (`smithy_query(query="operation",operation="ListFirewallRuleGroups")`)
/// — so, for the same reason as [`FIREWALL_DOMAIN_LIST_LIST_SCOPE`], the scope
/// is a constant that names the operation.
pub const FIREWALL_RULE_GROUP_LIST_SCOPE: &str = "ListFirewallRuleGroups/all/v1";

/// The largest page `ListFirewallConfigs` may be asked for, from the range the
/// model puts on its `MaxResults` (5..10,
/// `smithy_query(query="operation",operation="ListFirewallConfigs")`), which is
/// narrower than the usual 1..100 the other list operations carry.
pub const MAX_FIREWALL_CONFIGS_PER_PAGE: i32 = 10;

/// The cursor scope of one `ListResolverConfigs` query.
///
/// `ListResolverConfigs` models no filter either — only `MaxResults` and
/// `NextToken` (`smithy_query(query="operation",operation="ListResolverConfigs")`)
/// — so, for the same reason as [`FIREWALL_DOMAIN_LIST_LIST_SCOPE`], the scope
/// is a constant that names the operation.
pub const RESOLVER_CONFIG_LIST_SCOPE: &str = "ListResolverConfigs/all/v1";

/// One page of a `@paginated` list operation.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The cursor that resumes where this page stopped, or `None` on the last
    /// page.
    pub next_token: Option<String>,
}

/// The checksum carried inside a pagination cursor.
fn page_token_digest(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN.as_bytes());
    hasher.update(b":");
    hasher.update(payload.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Mint the opaque cursor that resumes `scope`'s collection at `offset`.
///
/// `scope` names the collection being paged (it carries the endpoint id for a
/// per-endpoint collection, and the filter set for a filtered one), so a cursor
/// minted for one collection is not accepted for another.
pub fn mint_page_token(scope: &str, offset: usize) -> String {
    let payload = format!("{scope}:{offset}");
    let digest = page_token_digest(&payload);
    hex::encode(format!("{payload}:{digest}"))
}

/// Read a cursor this emulator minted for `scope`, or `None` for one it did not
/// — which a list operation answers with `InvalidNextTokenException`.
pub fn parse_page_token(scope: &str, token: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (payload, digest) = decoded.rsplit_once(':')?;
    if digest != page_token_digest(payload) {
        return None;
    }
    let (token_scope, offset) = payload.rsplit_once(':')?;
    if token_scope != scope {
        return None;
    }
    offset.parse().ok()
}

/// Cut one page out of `all`, honouring the `@paginated` contract: resume at the
/// cursor, truncate at `MaxResults`, and mint a cursor only when items remain.
///
/// `None` means `next_token` is not a cursor this emulator minted for `scope`.
pub fn paginate<T>(
    all: Vec<T>,
    scope: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<Page<T>> {
    paginate_up_to(all, scope, max_results, next_token, MAX_RESULTS_RANGE.1)
}

/// [`paginate`] for a list operation whose `MaxResults` range is not the usual
/// 1..100: `largest_page` is the maximum THAT operation's range declares, so a
/// page is never cut shorter than a caller the model lets ask for more.
pub fn paginate_up_to<T>(
    all: Vec<T>,
    scope: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
    largest_page: i32,
) -> Option<Page<T>> {
    let offset = match next_token {
        Some(token) => parse_page_token(scope, token)?,
        None => 0,
    };
    let page_size = max_results
        .map(|requested| requested.clamp(MAX_RESULTS_RANGE.0, largest_page.max(MAX_RESULTS_RANGE.0)))
        .and_then(|bounded| usize::try_from(bounded).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE);

    // One item past the page is enough to know whether a cursor is owed, so the
    // allocation is bounded by the page size rather than by the collection.
    let mut items: Vec<T> = all
        .into_iter()
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
    let next_token = if items.len() > page_size {
        items.truncate(page_size);
        Some(mint_page_token(scope, offset.saturating_add(page_size)))
    } else {
        None
    };
    Some(Page { items, next_token })
}
