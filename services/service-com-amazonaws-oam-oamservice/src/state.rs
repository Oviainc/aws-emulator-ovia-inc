// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-oam-oamservice service.
//!
//! OAM (CloudWatch Observability Access Manager) has two resources, and every
//! operation in this slice answers from the one stored copy of them:
//!
//! * a **sink** — the attachment point a monitoring account owns. `CreateSink`'s
//!   own documentation says "Each account can contain one sink per Region", so a
//!   sink is stored with the region it was created in and at most one sink exists
//!   per region.
//! * a **link** — one source account's attachment to a sink. `CreateLink`'s
//!   documentation says each sink can carry as many as 100,000 source accounts and
//!   each source account can link to as many as five monitoring accounts.
//!
//! Both are keyed by the random id that is the last segment of their ARN, with two
//! secondary indexes: `sink_name_index` so a sink resolves by name as well as by id
//! or ARN, and `links_by_sink` so `ListAttachedLinks` enumerates one sink's links in
//! a stable (sorted) order and `DeleteSink` can see that links are still attached.
//!
//! Every resource carries the region it was created in and is only ever resolved
//! within the region the request was signed for: an ARN names its region, and a
//! regional endpoint only ever sees its own region's resources.
//!
//! A sink also carries the resource policy `PutSinkPolicy` attaches to it and
//! `GetSinkPolicy` reports, held on the sink's own record so the two answer from one
//! stored copy of it and deleting the sink takes its policy with it.
//!
//! A few stored members are not reported by any operation in this slice — a link's
//! `sink_id` and `created_at`, and a sink's `owner_account` and `created_at`. They are
//! written where the resource is created so any later slice that reads them reads the
//! same record rather than a second copy of it, and so the account that owns a sink is
//! the one `CreateLink` compares a source account against.
//!
//! Tags live on the resource record itself, so `TagResource` and `UntagResource` write
//! through the very map `CreateSink`/`CreateLink` seeded and `GetSink`/`GetLink` and
//! `ListTagsForResource` report
//! ([`ComAmazonawsOamOamserviceState::resource_tags`]): a resource has ONE tag set
//! however it is reached.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::LinkConfiguration;

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name `oam`).
pub const ARN_SERVICE: &str = "oam";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion a sink ARN carries before its id.
pub const SINK_ARN_RESOURCE_PREFIX: &str = "sink/";

/// The resource portion a link ARN carries before its id.
pub const LINK_ARN_RESOURCE_PREFIX: &str = "link/";

/// The variables `LabelTemplate` may carry, LONGEST FIRST.
///
/// From the `LabelTemplate` member's own documentation
/// (`smithy_query(query="doc",shape="CreateLinkInput",member="LabelTemplate")`):
/// `$AccountName`, `$AccountEmail` and `$AccountEmailNoDomain`. The order matters —
/// `$AccountEmail` is a prefix of `$AccountEmailNoDomain`, so substituting the short
/// one first would leave `NoDomain` behind.
pub const LABEL_TEMPLATE_VARIABLES: &[&str] =
    &["$AccountEmailNoDomain", "$AccountEmail", "$AccountName"];

/// The most links one source account may hold.
///
/// From `CreateLink`'s own documentation: "Each source account can be linked to as
/// many as five monitoring accounts."
pub const MAX_LINKS_PER_SOURCE_ACCOUNT: usize = 5;

/// The most sinks this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: AWS's quota is one sink per Region
/// (enforced in `CreateSink`), and the number of regions a caller can sign for is
/// caller-controlled, so this cap is what keeps the store finite. A create that
/// would exceed it is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_SINKS: usize = 100;

/// The most links this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota: AWS documents 100,000 source accounts
/// per sink, which this emulator does not have the memory to hold, so this tighter
/// cap is what keeps the store finite. A create that would exceed it is refused with
/// the modelled `ServiceQuotaExceededException`.
pub const MAX_LINKS: usize = 10_000;

/// The most tags one resource may carry, sinks and links alike.
///
/// From `TooManyTagsException`'s own documentation ("A resource can have no more
/// than 50 tags.") and `TagResource`'s ("You can associate as many as 50 tags with
/// a resource."): the bound is on what the RESOURCE holds, not on what one request
/// carries — the per-request bound is the modelled `TagMapInput` `@length(0..50)`,
/// which the centralized walk enforces before a handler runs.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One stored sink: every member `GetSink` and `CreateSink` report, plus the
/// resource policy `Put/GetSinkPolicy` will attach to it and the tags the tagging
/// operations read off its ARN.
#[derive(Debug, Clone, Default)]
pub struct Sink {
    /// The random id AWS allocates: the ARN's last segment and this record's key.
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The region this sink was created in — the ARN's region field.
    pub region: String,
    /// Which account OWNS this sink: the monitoring account that created it, read
    /// from the creating request's credential the way a link's source account is
    /// (see [`caller_source_account`]). A link may not attach a source account to a
    /// sink this same account owns, so the owner is stored on the sink rather than
    /// recomputed from one request's credential. Reported by no operation.
    pub owner_account: String,
    pub tags: BTreeMap<String, String>,
    /// The sink policy, present only once a `PutSinkPolicy` has stored one.
    pub policy: Option<String>,
    /// When this sink was created, in whole seconds since the Unix epoch.
    pub created_at: i64,
}

/// One stored link: every member `GetLink`, `CreateLink` and `ListAttachedLinks`
/// report, plus the back-reference to the sink it is attached to.
#[derive(Debug, Clone, Default)]
pub struct Link {
    /// The random id AWS allocates: the ARN's last segment and this record's key.
    pub id: String,
    pub arn: String,
    /// `LabelTemplate` with its variables resolved — what the service reports as
    /// `Label`.
    pub label: String,
    /// The template exactly as the caller sent it.
    pub label_template: String,
    pub resource_types: Vec<String>,
    pub sink_arn: String,
    pub sink_id: String,
    /// The region this link was created in — the ARN's region field.
    pub region: String,
    /// Which source account this link attaches to the sink. See
    /// [`caller_source_account`].
    pub source_account: String,
    pub link_configuration: Option<LinkConfiguration>,
    pub tags: BTreeMap<String, String>,
    /// When this link was created, in whole seconds since the Unix epoch.
    pub created_at: i64,
}

/// The tag map a response carries, as the wire shape wants it.
///
/// `include` is false for an operation whose caller did not ask for tags
/// (`IncludeTags` absent or false). An empty map is omitted rather than reported as
/// `{}`: a resource carrying no tags has none to report, and `Tags` is an optional
/// output member precisely so it can be left out.
fn tags_output(tags: &BTreeMap<String, String>, include: bool) -> Option<HashMap<String, String>> {
    (include && !tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

impl Sink {
    /// This sink's `Tags`, reported only when the caller asked for them.
    pub fn tags_output(&self, include: bool) -> Option<HashMap<String, String>> {
        tags_output(&self.tags, include)
    }
}

impl Link {
    /// This link's `Tags`, reported only when the caller asked for them.
    pub fn tags_output(&self, include: bool) -> Option<HashMap<String, String>> {
        tags_output(&self.tags, include)
    }
}

/// Which of this service's two resources an ARN names, with the id its record is
/// keyed by.
///
/// "Both sinks and links support tagging" (`ListTagsForResource`'s own
/// documentation), and the two live in two maps, so resolving a tagging request's
/// `ResourceArn` has to say WHICH map holds the record as well as where in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// The sink stored under this id.
    Sink(String),
    /// The link stored under this id.
    Link(String),
}

/// Per-service state for com-amazonaws-oam-oamservice. A handler reaches it through
/// [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOamOamserviceState {
    /// Every sink, keyed by its id. A `BTreeMap` so an enumeration walks a stable,
    /// total order.
    pub sinks: BTreeMap<String, Sink>,
    /// Every link, keyed by its id, for the same reason.
    pub links: BTreeMap<String, Link>,
    /// (region, sink name) -> sink id, so an `Identifier` resolves by name and
    /// `CreateSink` can see that the name is already in use.
    pub sink_name_index: HashMap<(String, String), String>,
    /// Sink ARN -> the ids of the links attached to it, sorted, giving
    /// `ListAttachedLinks` a deterministic enumeration order (so a page token can
    /// name the id the next page starts at) and `DeleteSink` its "still has links"
    /// check.
    pub links_by_sink: BTreeMap<String, BTreeSet<String>>,
    /// How many resource ids this store has minted. Every mint consumes one step of
    /// it, so two resources can never be handed the same id; see
    /// [`ComAmazonawsOamOamserviceState::fresh_id`].
    pub minted_ids: u64,
}

impl ComAmazonawsOamOamserviceState {
    /// The sink in `region`, if that region holds one.
    ///
    /// At most one ever does: `CreateSink` refuses a second one, the way AWS does
    /// ("Each account can contain one sink per Region").
    pub fn sink_in_region(&self, region: &str) -> Option<&Sink> {
        self.sinks.values().find(|sink| sink.region == region)
    }

    /// The id of the sink `identifier` names in `region`, accepting any of the forms
    /// the modelled `ResourceIdentifier` pattern admits: the full ARN, the bare id,
    /// or the sink's name.
    ///
    /// An ARN must match the stored ARN in EVERY field, so an ARN naming another
    /// partition, account or region resolves to nothing rather than to the sink that
    /// happens to share its id segment. Every form is resolved within `region`: a
    /// regional endpoint sees only its own region's sinks.
    pub fn resolve_sink_id(&self, region: &str, identifier: &str) -> Option<String> {
        if let Some(id) = id_from_arn(identifier, SINK_ARN_RESOURCE_PREFIX) {
            return self
                .sinks
                .get(id)
                .filter(|sink| sink.arn == identifier && sink.region == region)
                .map(|sink| sink.id.clone());
        }
        if let Some(id) = self
            .sink_name_index
            .get(&(region.to_string(), identifier.to_string()))
        {
            return Some(id.clone());
        }
        self.sinks
            .get(identifier)
            .filter(|sink| sink.region == region)
            .map(|sink| sink.id.clone())
    }

    /// The sink `identifier` names in `region`, for a reporting operation.
    pub fn sink(&self, region: &str, identifier: &str) -> Option<&Sink> {
        let id = self.resolve_sink_id(region, identifier)?;
        self.sinks.get(&id)
    }

    /// Store a newly created sink and index it by name.
    pub fn insert_sink(&mut self, sink: Sink) {
        self.sink_name_index
            .insert((sink.region.clone(), sink.name.clone()), sink.id.clone());
        self.sinks.insert(sink.id.clone(), sink);
    }

    /// Remove the sink `id` keys, with its name index entry and its (by then empty)
    /// entry in [`Self::links_by_sink`]. `None` when no sink is stored under `id`.
    ///
    /// Takes the id rather than an identifier so a caller that has already resolved
    /// one — `DeleteSink`, which must read the sink's ARN to check for attached links
    /// first — removes the very record it looked at. Refusing a sink that still has
    /// links is the caller's job: this removes what it is told to and would leave the
    /// links behind with no sink.
    pub fn remove_sink_by_id(&mut self, id: &str) -> Option<Sink> {
        let sink = self.sinks.remove(id)?;
        self.sink_name_index
            .remove(&(sink.region.clone(), sink.name.clone()));
        self.links_by_sink.remove(&sink.arn);
        Some(sink)
    }

    /// The id of the link `identifier` names in `region`: its full ARN or its bare
    /// id. A link has no name, so there is no third form. Both forms are resolved
    /// within `region`, as a sink's are.
    pub fn resolve_link_id(&self, region: &str, identifier: &str) -> Option<String> {
        if let Some(id) = id_from_arn(identifier, LINK_ARN_RESOURCE_PREFIX) {
            return self
                .links
                .get(id)
                .filter(|link| link.arn == identifier && link.region == region)
                .map(|link| link.id.clone());
        }
        self.links
            .get(identifier)
            .filter(|link| link.region == region)
            .map(|link| link.id.clone())
    }

    /// The link `identifier` names in `region`, for a reporting operation.
    pub fn link(&self, region: &str, identifier: &str) -> Option<&Link> {
        let id = self.resolve_link_id(region, identifier)?;
        self.links.get(&id)
    }

    /// Store a newly created link and attach it to its sink's index.
    pub fn insert_link(&mut self, link: Link) {
        self.links_by_sink
            .entry(link.sink_arn.clone())
            .or_default()
            .insert(link.id.clone());
        self.links.insert(link.id.clone(), link);
    }

    /// Remove the link `identifier` names in `region`, detaching it from its sink.
    /// `None` when nothing matched.
    pub fn remove_link(&mut self, region: &str, identifier: &str) -> Option<Link> {
        let id = self.resolve_link_id(region, identifier)?;
        let link = self.links.remove(&id)?;
        if let Some(attached) = self.links_by_sink.get_mut(&link.sink_arn) {
            attached.remove(&link.id);
            if attached.is_empty() {
                self.links_by_sink.remove(&link.sink_arn);
            }
        }
        Some(link)
    }

    /// The ids of the links attached to the sink `sink_arn` names, in sorted order.
    pub fn link_ids_for_sink(&self, sink_arn: &str) -> Option<&BTreeSet<String>> {
        self.links_by_sink.get(sink_arn)
    }

    /// The link `source_account` already has to the sink `sink_arn` names, if any —
    /// the one `CreateLink` refuses to duplicate.
    pub fn link_from_source_to_sink(&self, source_account: &str, sink_arn: &str) -> Option<&Link> {
        self.link_ids_for_sink(sink_arn)?
            .iter()
            .filter_map(|id| self.links.get(id))
            .find(|link| link.source_account == source_account)
    }

    /// How many links `source_account` holds, across every sink.
    pub fn links_from_source(&self, source_account: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.source_account == source_account)
            .count()
    }

    /// Which resource `arn` names in `region`, or `None` when no sink or link of this
    /// region is stored under that exact ARN.
    ///
    /// A tagging operation's `ResourceArn` is an ARN and nothing else (its modelled
    /// `^arn:(\w|-)+:oam:.+:.+:.+` pattern admits no other form), so resolution is by
    /// FULL ARN equality against the stored record: an ARN naming another partition,
    /// account or region resolves to nothing rather than to the resource that happens
    /// to share its id segment. Scoped to `region` for the same reason every other
    /// lookup here is — a regional endpoint only ever sees its own region's resources.
    pub fn tagged_resource(&self, region: &str, arn: &str) -> Option<TaggedResource> {
        if let Some(id) = id_from_arn(arn, SINK_ARN_RESOURCE_PREFIX) {
            return self
                .sinks
                .get(id)
                .filter(|sink| sink.arn == arn && sink.region == region)
                .map(|sink| TaggedResource::Sink(sink.id.clone()));
        }
        if let Some(id) = id_from_arn(arn, LINK_ARN_RESOURCE_PREFIX) {
            return self
                .links
                .get(id)
                .filter(|link| link.arn == arn && link.region == region)
                .map(|link| TaggedResource::Link(link.id.clone()));
        }
        None
    }

    /// The tags stored on the resource `arn` names in `region`, for a reporting
    /// operation; `None` when that resource is not stored.
    ///
    /// Answers the ONE stored tag map — the same one `CreateSink`/`CreateLink` wrote
    /// and `GetSink`/`GetLink` report — so there is never a second copy of a
    /// resource's tags to drift from it.
    pub fn resource_tags(&self, region: &str, arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.tagged_resource(region, arn)? {
            TaggedResource::Sink(id) => self.sinks.get(&id).map(|sink| &sink.tags),
            TaggedResource::Link(id) => self.links.get(&id).map(|link| &link.tags),
        }
    }

    /// The same stored tag map, for a tagging operation to write THROUGH; `None` when
    /// that resource is not stored — which is the 404, never a tag map conjured for an
    /// absent resource.
    ///
    /// Because it is the resource's own map, a write is visible to every later read of
    /// that one record (`ListTagsForResource`, and `GetSink`/`GetLink` with
    /// `IncludeTags`).
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match self.tagged_resource(region, arn)? {
            TaggedResource::Sink(id) => self.sinks.get_mut(&id).map(|sink| &mut sink.tags),
            TaggedResource::Link(id) => self.links.get_mut(&id).map(|link| &mut link.tags),
        }
    }

    /// A fresh id no stored sink is using, or `None` when one could not be found.
    ///
    /// Takes `&mut self` because minting an id CONSUMES one step of
    /// [`Self::minted_ids`]: two creates can never be handed the same id, even within
    /// one clock tick.
    pub fn fresh_sink_id(&mut self) -> Option<String> {
        self.fresh_id(|store, candidate| store.sinks.contains_key(candidate))
    }

    /// A fresh id no stored link is using, or `None` when one could not be found.
    pub fn fresh_link_id(&mut self) -> Option<String> {
        self.fresh_id(|store, candidate| store.links.contains_key(candidate))
    }

    /// A fresh resource id that `taken` does not already claim.
    ///
    /// AWS calls this "the random ID string that Amazon Web Services generated as
    /// part of the sink ARN", so the id is opaque and carries the UUID shape the
    /// service hands out. [`mint_id`] derives it from this process's salt and this
    /// counter — never from the request, so nothing a caller sends can choose or
    /// predict a resource's id. The counter makes every id distinct by construction;
    /// the store is asked anyway, with a bounded number of attempts so this can never
    /// spin. `None` rather than an unchecked id when every attempt collided: the
    /// caller answers a modelled server error instead of overwriting a stored record.
    fn fresh_id(&mut self, taken: impl Fn(&Self, &str) -> bool) -> Option<String> {
        for _ in 0..16 {
            self.minted_ids = self.minted_ids.wrapping_add(1);
            let candidate = mint_id(self.minted_ids);
            if !taken(self, &candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

/// The 128 bits an id is built from: SHA-256 over this process's salt and the store's
/// mint counter, truncated to 16 bytes.
///
/// The salt is what makes an id unguessable from outside — a caller cannot see how
/// many ids a store has minted, and could not compute the next one if it could — and
/// the counter is what makes it unique. Neither is request-derived, so an id never
/// echoes client input. SHA-256 comes from the `sha2` crate rather than being
/// hand-rolled.
fn mint_id_bytes(ordinal: u64) -> [u8; 16] {
    use sha2::Digest as _;
    let mut digest = sha2::Sha256::new();
    digest.update(process_salt());
    digest.update(ordinal.to_be_bytes());
    let full = digest.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&full[..16]);
    bytes
}

/// A fresh opaque resource id, in the 8-4-4-4-12 hexadecimal form this service's ids
/// carry, with the version and variant bits a version-4 UUID sets.
fn mint_id(ordinal: u64) -> String {
    let mut bytes = mint_id_bytes(ordinal);
    // RFC 4122 places the version in the high nibble of byte 6 and the variant in the
    // top two bits of byte 8; setting them is what makes this a well-formed UUID
    // rather than 32 loose hex digits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// This process's id salt: 16 bytes taken ONCE, from the operating system's entropy
/// by way of [`std::collections::hash_map::RandomState`], whose keys the standard
/// library seeds from the OS. Two `RandomState`s hash one input to two different
/// values, so two passes give two independent halves.
///
/// Taken once and reused, which is what lets the mint counter alone guarantee that
/// two ids from one process differ.
fn process_salt() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    static SALT: ::std::sync::OnceLock<[u8; 16]> = ::std::sync::OnceLock::new();
    *SALT.get_or_init(|| {
        let mut salt = [0u8; 16];
        for (half, chunk) in salt.chunks_mut(8).enumerate() {
            let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u64(half as u64);
            chunk.copy_from_slice(&hasher.finish().to_be_bytes());
        }
        salt
    })
}

/// The id segment of one of this service's ARNs, or `None` when `identifier` is not
/// an ARN of this service's `prefix` resource.
fn id_from_arn<'a>(identifier: &'a str, prefix: &str) -> Option<&'a str> {
    if !identifier.starts_with("arn:") {
        return None;
    }
    let mut fields = identifier.splitn(6, ':');
    let (_arn, _partition, service, _region, _account, resource) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if service != ARN_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(prefix)?;
    (!id.is_empty()).then_some(id)
}

/// The ARN of the sink `id` in `region`.
pub fn sink_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SINK_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the link `id` in `region`.
pub fn link_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LINK_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// `LabelTemplate` with its variables resolved — the `Label` the service reports.
///
/// Every variable resolves to the account id. That is what the member's own
/// documentation says the service does where the account's name and email address
/// are not available to it ("the `$AccountName`, `$AccountEmail`, and
/// `$AccountEmailNoDomain` variables all resolve as *account-id* instead of the
/// specified variable"), and this emulator has one canonical account
/// ([`engine::EMULATOR_ACCOUNT_ID`]) with no Organizations name or email address
/// behind it. Text around a variable is kept, so a custom label is reported
/// unchanged.
pub fn resolve_label(label_template: &str) -> String {
    let account = ::engine::emulator_account_id();
    LABEL_TEMPLATE_VARIABLES
        .iter()
        .fold(label_template.to_string(), |label, variable| {
            label.replace(variable, account)
        })
}

/// The first entry of `resource_types` the model's `ResourceType` enum does not
/// declare, or `None` when every one of them is a modelled value.
///
/// The catalog is the generated [`crate::ops::types::RESOURCE_TYPE_VALUES`], which
/// carries exactly the values
/// `smithy_query(query="shape",shape="ResourceType")` reports.
pub fn unmodelled_resource_type(resource_types: &[String]) -> Option<&str> {
    resource_types
        .iter()
        .map(String::as_str)
        .find(|supplied| !crate::ops::types::RESOURCE_TYPE_VALUES.contains(supplied))
}

/// The lower bound of the modelled `ResourceTypesInput` `@length`, which the list
/// declares as 1..50 with `@uniqueItems` over elements targeting the `ResourceType`
/// enum (`smithy_query(query="shape",shape="ResourceTypesInput")`).
pub const MIN_RESOURCE_TYPES: usize = 1;

/// The upper bound of that same `@length`.
pub const MAX_RESOURCE_TYPES: usize = 50;

/// Why `resource_types` is not a `ResourceTypesInput` the model admits, or `None` when
/// it is one.
///
/// Every clause is a modelled constraint of that list, and the refusal names the
/// member, the rule and what the request sent for it, so a caller can fix the request
/// from the text alone. For an operation whose `ResourceTypes` the centralized
/// pre-handler walk sees, this is a second check of what that walk already refuses;
/// for `UpdateLink`, whose authorization decision comes first, it is the only one (see
/// [`crate::ops::types::UpdateLinkRequest`]).
pub fn resource_types_rejection(resource_types: &[String]) -> Option<String> {
    if resource_types.len() < MIN_RESOURCE_TYPES {
        return Some(format!(
            "ResourceTypes must name at least {MIN_RESOURCE_TYPES} resource type"
        ));
    }
    if resource_types.len() > MAX_RESOURCE_TYPES {
        return Some(format!(
            "ResourceTypes names {} resource types, past the {MAX_RESOURCE_TYPES} this list \
             admits",
            resource_types.len()
        ));
    }
    let mut seen = BTreeSet::new();
    if resource_types
        .iter()
        .any(|resource_type| !seen.insert(resource_type.as_str()))
    {
        return Some("ResourceTypes must not name one resource type twice".to_string());
    }
    if let Some(unmodelled) = unmodelled_resource_type(resource_types) {
        // The offending value is caller-controlled text, so only its first characters
        // are echoed back.
        let shown: String = unmodelled.chars().take(64).collect();
        return Some(format!(
            "ResourceTypes contains '{shown}', which is not one of [{}]",
            crate::ops::types::RESOURCE_TYPE_VALUES.join(", ")
        ));
    }
    None
}

/// The most bytes of sink policy document this emulator will attach to a sink.
///
/// Emulator resource guard, NOT an AWS quota: the modelled `SinkPolicy` string
/// (`smithy_query(query="shape",shape="SinkPolicy")`) carries no `@length`, so
/// nothing else bounds how much caller-supplied text one stored sink can hold. A
/// longer document is refused with the modelled `InvalidParameterException` instead
/// of being stored.
pub const MAX_SINK_POLICY_BYTES: usize = 32 * 1024;

/// The message a refused sink policy document is reported with.
///
/// The service judges the document as a whole rather than one member of it, and the
/// recorded behaviour of every refused `PutSinkPolicy` carries this one fixed
/// string, so the refusal names nothing from the request — the document is
/// caller-controlled text and never echoed back.
pub const POLICY_VALIDATION_FAILED: &str = "Policy validation failed";

/// The `Effect` values a policy statement may carry.
///
/// The IAM policy grammar declares exactly these two, case-sensitively, for every
/// statement of every policy document.
pub const POLICY_EFFECTS: &[&str] = &["Allow", "Deny"];

/// The message this service's not-found refusals carry.
///
/// Fixed and identifier-independent: the model states no text for
/// `ResourceNotFoundException`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` reports only
/// `Message` / `x-amzn-ErrorType` at HTTP 404), and every not-found path the
/// behavioural recording of this service covers answers this one constant string. A
/// not-found refusal therefore never embeds the identifier the request asked about.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The message a `CreateLink` naming a sink the calling account itself owns is refused
/// with.
///
/// A monitoring link joins TWO accounts — "Creates a link between a source account and
/// a sink that you have created in a monitoring account" (`CreateLink`'s own
/// documentation) — so a sink the caller already owns is not a sink it can link to.
/// The refusal is about that one fact and so has no variable parts: nothing of the
/// request, and nothing of what is stored, reaches it.
pub const SAME_ACCOUNT_LINK_MESSAGE: &str = "Cannot create a link to a sink in the same account";

/// The message a `DeleteSink` on a sink that still has links attached is refused with.
///
/// `DeleteSink`'s own documentation states the rule in one sentence — "You must delete
/// all links to a sink before you can delete that sink." — and that rule is the whole
/// of what the refusal reports, so neither the identifier the request supplied nor how
/// many links happen to be attached reaches the message.
pub const SINK_HAS_LINKS_MESSAGE: &str =
    "You must delete all links to a sink before you can delete that sink.";

/// The most sinks one account may hold in one Region.
///
/// From `CreateSink`'s own documentation: "Each account can contain one sink per
/// Region. If you delete a sink, you can then create a new one in that Region." The
/// bound is on the account/Region, NOT on the sink's name: a create is refused
/// because the one slot is taken, whatever name it asks for, and deleting the stored
/// sink frees the slot again.
pub const MAX_SINKS_PER_REGION: usize = 1;

/// The refusal message a `CreateSink` earns when the account/Region already holds its
/// one sink.
///
/// Built from the quota and the account the request is served for, which is the whole
/// of what the refusal is about; nothing of the request (least of all the `Name`,
/// which played no part in the decision) reaches it.
pub fn sink_quota_message() -> String {
    format!(
        "Reached account sink max({MAX_SINKS_PER_REGION}) for account({}).",
        ::engine::emulator_account_id()
    )
}

/// Which of this service's two resource kinds an operation's identifier may name.
///
/// Every operation that takes a `ResourceIdentifier` acts on ONE kind — `DeleteSink`
/// on a sink, `UpdateLink` on a link — and that is what its IAM action is authorized
/// against. See [`require_resource_kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Sink,
    Link,
}

impl ResourceKind {
    /// The resource portion an ARN of this kind carries before its id.
    pub fn arn_prefix(self) -> &'static str {
        match self {
            Self::Sink => SINK_ARN_RESOURCE_PREFIX,
            Self::Link => LINK_ARN_RESOURCE_PREFIX,
        }
    }

    /// The other kind — the one an operation acting on `self` is never authorized for.
    pub fn other(self) -> Self {
        match self {
            Self::Sink => Self::Link,
            Self::Link => Self::Sink,
        }
    }
}

/// The identity every request to this emulator is authenticated as: the one IAM user
/// of the single account it serves.
///
/// The emulator verifies a request's signature but has no IAM directory behind the
/// credential it was signed with, so there is one principal for an authorization
/// decision to name — the identity `sts:GetCallerIdentity` answers with for a
/// credential that names no session, which every other service of this emulator names
/// the same way.
pub fn caller_principal() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The ARN naming a resource KIND of this service in `region` rather than one resource
/// of it: the wildcard an authorization decision taken on the kind reports.
pub fn resource_kind_wildcard(region: &str, kind: ResourceKind) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{}*", kind.arn_prefix()))
}

/// How much of a caller-supplied identifier a refusal echoes.
///
/// The modelled `ResourceIdentifier` pattern admits 2048 characters, and the value
/// lands in a response message, so only the leading characters are echoed: a request
/// can never be amplified into an arbitrarily long refusal.
const MAX_ECHOED_IDENTIFIER: usize = 256;

/// The explicit-deny refusal for `operation` on the resource `identifier` names.
///
/// The message states the decision: WHO was refused (the one principal this emulator
/// serves), WHAT action was asked for (`oam:` plus the operation, the IAM action name
/// for it) and WHICH resource it was asked for (the identifier the request supplied),
/// and that a resource-based policy denied it. Every part is a fact of this request,
/// so nothing is kept from one and replayed into another.
pub fn explicit_deny(operation: &str, identifier: &str) -> crate::scaffolded::AccessDeniedException {
    let shown: String = identifier.chars().take(MAX_ECHOED_IDENTIFIER).collect();
    crate::scaffolded::AccessDeniedException::new(format!(
        "User: {} is not authorized to perform: {ARN_SERVICE}:{operation} on resource: {shown} \
         with an explicit deny in a resource-based policy",
        caller_principal()
    ))
}

/// How this service words a refusal for a required member the request left out.
///
/// The ONE wording for an absent member, whether the centralized walk found it before
/// a handler ran or a handler enforced it itself after taking an authorization
/// decision first: both render the framework's clause for it, so the two cannot drift
/// into two sentences for one refusal.
pub fn missing_member_message(member: &str) -> String {
    ::aws_smithy_schema_server::constraints::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(member),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    }
    .message()
}

/// Refuse `identifier` when it is an ARN of this service naming the resource kind
/// `operation` does NOT act on.
///
/// An IAM action applies to one resource kind, so an identifier naming the other kind
/// cannot be authorized for it however the store happens to be populated: the
/// authorization decision is taken on the resource the ARN names, BEFORE any lookup,
/// and the refusal therefore discloses nothing about whether anything is stored under
/// it. Any other form the modelled pattern admits — a bare id, a sink's name, an ARN
/// of the kind this operation does act on — is a resource this action can apply to,
/// and is resolved against the store as usual.
pub fn require_resource_kind(
    operation: &str,
    identifier: &str,
    kind: ResourceKind,
) -> Result<(), crate::scaffolded::AccessDeniedException> {
    if id_from_arn(identifier, kind.other().arn_prefix()).is_some() {
        return Err(explicit_deny(operation, identifier));
    }
    Ok(())
}

/// Why `policy` is not a sink policy document this service will attach, or `None`
/// when it is one.
///
/// A sink policy is an IAM **resource**-based policy: `PutSinkPolicy`'s `Policy`
/// member is documented as "The JSON policy to use… The policy must be in JSON
/// string format", and the operation's own documentation describes it as granting
/// permissions to source accounts. So the document must be a JSON object whose
/// `Statement` is a statement object or a non-empty array of them, and each
/// statement must say whether it allows or denies (`Effect`, exactly one of
/// [`POLICY_EFFECTS`]), WHO it applies to (`Principal` or `NotPrincipal`) and WHAT
/// it applies to (`Action` or `NotAction`). The principal is what makes a document a
/// resource policy rather than an identity policy, so a statement without one grants
/// nobody anything and the whole document is refused. `Resource` is not required:
/// a policy attached to a sink is already scoped to that sink.
///
/// Parsed with `serde_json`, so what judges the document is a real JSON parser and
/// never a match on its text. A document this refuses is not stored at all, so a
/// later `GetSinkPolicy` can never hand back something the service would not have
/// accepted.
pub fn sink_policy_rejection(policy: &str) -> Option<String> {
    if policy.len() > MAX_SINK_POLICY_BYTES {
        return Some(format!(
            "Policy is {} bytes, past the {MAX_SINK_POLICY_BYTES} bytes this emulator stores",
            policy.len()
        ));
    }
    let refused = || Some(POLICY_VALIDATION_FAILED.to_string());
    let Ok(document) = serde_json::from_str::<serde_json::Value>(policy) else {
        return refused();
    };
    let Some(statements) = document.as_object().and_then(policy_statements) else {
        return refused();
    };
    if statements
        .iter()
        .any(|statement| !is_resource_policy_statement(statement))
    {
        return refused();
    }
    None
}

/// The statements of a policy document, or `None` when its `Statement` element is
/// absent or is neither a statement object nor a non-empty array of them.
fn policy_statements(
    document: &serde_json::Map<String, serde_json::Value>,
) -> Option<Vec<&serde_json::Value>> {
    match document.get("Statement")? {
        serde_json::Value::Array(items) if !items.is_empty() => Some(items.iter().collect()),
        statement @ serde_json::Value::Object(_) => Some(vec![statement]),
        _ => None,
    }
}

/// Whether one statement is a resource-policy statement, as
/// [`sink_policy_rejection`] states the rule.
fn is_resource_policy_statement(statement: &serde_json::Value) -> bool {
    let Some(statement) = statement.as_object() else {
        return false;
    };
    let effect_declared = statement
        .get("Effect")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|effect| POLICY_EFFECTS.contains(&effect));
    effect_declared
        && names_something(statement, "Principal", "NotPrincipal")
        && names_something(statement, "Action", "NotAction")
}

/// Whether a statement carries `element` or its negated form `negated`, with a value
/// that names something.
///
/// A policy element names something when it is a non-empty string (`"*"`), a
/// non-empty list of them, or a non-empty object keyed by principal type
/// (`{"AWS": [..]}`); an empty or absent one names nobody and nothing.
fn names_something(
    statement: &serde_json::Map<String, serde_json::Value>,
    element: &str,
    negated: &str,
) -> bool {
    [element, negated]
        .iter()
        .filter_map(|name| statement.get(*name))
        .any(|value| match value {
            serde_json::Value::String(text) => !text.trim().is_empty(),
            serde_json::Value::Array(items) => !items.is_empty(),
            serde_json::Value::Object(map) => !map.is_empty(),
            _ => false,
        })
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not shaped like a region is refused here and the default is used
/// instead: a generated ARN can never carry text an ARN's region field could not
/// hold.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// What a stored [`Link::source_account`] starts with.
///
/// It marks the value as an opaque per-caller identity derived from the request's
/// credential — NOT an AWS account number, which in this emulator is the one
/// canonical [`engine::EMULATOR_ACCOUNT_ID`] every ARN carries.
pub const SOURCE_IDENTITY_PREFIX: &str = "akid:";

/// Which source account this request speaks for.
///
/// A link belongs to the source account that created it: AWS reads that account
/// from the request's credentials, and so does this. Every ARN a generated emulator
/// emits carries the one canonical account id
/// ([`engine::EMULATOR_ACCOUNT_ID`]), so the account a request speaks for cannot be
/// a distinct account NUMBER here; it is the request's own SigV4 access key id. Two
/// requests signed with the same credential are the same source account — which is
/// what makes `CreateLink`'s modelled `ConflictException` (one link per source
/// account per sink) and its five-links-per-source-account quota observable.
///
/// The id is caller-controlled text, so it is hex-encoded rather than filtered:
/// encoding is INJECTIVE, so two credentials that differ at all stay two source
/// accounts (dropping punctuation would collapse `AKID-1` and `AKID1` into one and
/// refuse the second one's link), and the stored value is bounded and holds nothing
/// but hex. The encoding is of the leading bytes, so a pathologically long header
/// cannot grow the store.
///
/// The value is prefixed with [`SOURCE_IDENTITY_PREFIX`] so it can never be read as
/// an AWS account NUMBER: it is an opaque identity for "which caller", and nothing
/// but equality is ever asked of it. It is deliberately reported by no operation.
pub fn caller_source_account(parts: &::http::request::Parts) -> String {
    let raw = ::engine::extract_akid(&parts.headers);
    let bounded: String = raw.chars().take(128).collect();
    if bounded.is_empty() {
        return format!("{SOURCE_IDENTITY_PREFIX}unsigned");
    }
    format!("{SOURCE_IDENTITY_PREFIX}{}", hex::encode(bounded))
}

/// Now, in whole seconds since the Unix epoch.
///
/// Saturating rather than truncating: the conversion is fallible in principle (the
/// clock reports a `u64`), and a clock far past the `i64` range must not wrap into a
/// negative creation time.
pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOamOamserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOamOamserviceState>)
}
