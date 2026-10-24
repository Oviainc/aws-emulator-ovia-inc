// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-imagebuilder-imagebuilder service.
//!
//! The first resource this store holds is the **infrastructure configuration**: the
//! environment Image Builder builds and tests an image in. Every configuration is
//! stored exactly once, keyed by its own ARN, so `CreateInfrastructureConfiguration`
//! writes the record that `GetInfrastructureConfiguration` reads back.
//!
//! Two facts about that resource shape the store:
//!
//! * The ARN is derived from the NAME, not minted independently — the modelled
//!   `name` member documents "Image Builder generates the infrastructure
//!   configuration ARN from a normalized form of the name, so names that differ only
//!   in case, spaces, or underscores count as the same name", and
//!   "Infrastructure configuration names must be unique to your account in each
//!   Amazon Web Services Region". [`normalized_name`] is that one normalization, so
//!   the uniqueness test and the ARN agree by construction.
//! * The service is REGIONAL (its endpoints are `imagebuilder.<region>.<dns-suffix>`),
//!   so a configuration belongs to the Region its ARN names and a regional endpoint
//!   reaches only the configurations of its own Region
//!   ([`ComAmazonawsImagebuilderImagebuilderState::infrastructure_configuration`]).
//!
//! `clientToken` is an idempotency token: a repeat of a request that already
//! succeeded answers with the ORIGINAL response, and the same token carrying
//! different creation parameters is a mismatch. Both need the first request's
//! parameters and its answer, so a successful create files a
//! [`CreateInfrastructureConfigurationReceipt`] beside the record.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::ops::types::{
    CreateInfrastructureConfigurationInput, InfrastructureConfiguration, InstanceMetadataOptions,
    Logging, ParentInstanceConfiguration, Placement,
};

/// This service's ARN namespace and SigV4 signing name (`arnNamespace` / `sigv4Name`
/// of `smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "imagebuilder";

/// The region used when a request carries no SigV4 credential scope to read one from;
/// the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion an infrastructure-configuration ARN carries before its name,
/// from the modelled `InfrastructureConfigurationArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):infrastructure-configuration/[a-z0-9-_]+$`.
pub const INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX: &str = "infrastructure-configuration/";

/// Applied when `CreateInfrastructureConfiguration` omits
/// `terminateInstanceOnFailure`, whose own documentation says "Defaults to true".
pub const DEFAULT_TERMINATE_INSTANCE_ON_FAILURE: bool = true;

/// Applied when `CreateInfrastructureConfiguration` omits
/// `terminateInstanceOnCompletion`, whose own documentation says "Defaults to true".
pub const DEFAULT_TERMINATE_INSTANCE_ON_COMPLETION: bool = true;

/// The metadata-tag keys a caller may not assign to the build and test instances.
///
/// From the `resourceTags` member's own documentation: "Tag keys can't begin with
/// `aws:` or match one of the following reserved keys: CreatedBy, Ec2ImageBuilderArn,
/// Name, or Tags." The `aws:` prefix is already refused by the modelled `TagKey`
/// pattern before a handler runs; these four are not, so they are refused here.
pub const RESERVED_RESOURCE_TAG_KEYS: &[&str] =
    &["CreatedBy", "Ec2ImageBuilderArn", "Name", "Tags"];

/// The most infrastructure configurations this emulator will hold.
///
/// An emulator resource guard, NOT an AWS quota: Image Builder publishes a per-account
/// limit but the model carries no value for it, so this cap is set well above any
/// realistic use and exists only so a caller-driven store cannot grow without bound.
pub const MAX_INFRASTRUCTURE_CONFIGURATIONS: usize = 1_000;

/// The most `clientToken` receipts this emulator keeps.
///
/// The receipt index's OWN bound, stated here rather than borrowed from the
/// configuration cap above: a receipt is read only while the caller that earned it is
/// still retrying, and nothing else ties its lifetime to the configuration it names —
/// so once a delete operation can remove a configuration without removing the receipt
/// that named it, THIS is the constant that still bounds the index. Past it the OLDEST
/// receipt is evicted ([`ComAmazonawsImagebuilderImagebuilderState::insert_infrastructure_configuration`]),
/// never the create.
pub const MAX_CREATE_RECEIPTS: usize = 2 * MAX_INFRASTRUCTURE_CONFIGURATIONS;

/// One stored infrastructure configuration: what a create wrote, which is what every
/// later read reports.
///
/// The list and map members are held as `Option`, so a read reports exactly what the
/// create stored — an omitted `instanceTypes` is reported as absent rather than as an
/// empty list.
#[derive(Debug, Clone)]
pub struct InfrastructureConfigurationRecord {
    /// This configuration's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub description: Option<String>,
    pub instance_types: Option<Vec<String>>,
    pub instance_profile_name: String,
    pub security_group_ids: Option<Vec<String>>,
    pub subnet_id: Option<String>,
    pub logging: Option<Logging>,
    pub key_pair: Option<String>,
    pub terminate_instance_on_failure: bool,
    pub terminate_instance_on_completion: bool,
    pub sns_topic_arn: Option<String>,
    pub resource_tags: Option<BTreeMap<String, String>>,
    pub instance_metadata_options: Option<InstanceMetadataOptions>,
    pub tags: Option<BTreeMap<String, String>>,
    pub placement: Option<Placement>,
    pub parent_instance: Option<ParentInstanceConfiguration>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    /// When it was last written. The create IS the last write until an update
    /// replaces it, so a freshly created configuration reports its creation instant.
    pub date_updated: String,
}

impl InfrastructureConfigurationRecord {
    /// This record as the modelled `InfrastructureConfiguration` a read reports.
    #[must_use]
    pub fn view(&self) -> InfrastructureConfiguration {
        InfrastructureConfiguration {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            instance_types: self.instance_types.clone(),
            instance_profile_name: Some(self.instance_profile_name.clone()),
            security_group_ids: self.security_group_ids.clone(),
            subnet_id: self.subnet_id.clone(),
            logging: self.logging.clone(),
            key_pair: self.key_pair.clone(),
            terminate_instance_on_failure: Some(self.terminate_instance_on_failure),
            sns_topic_arn: self.sns_topic_arn.clone(),
            date_created: Some(self.date_created.clone()),
            date_updated: Some(self.date_updated.clone()),
            resource_tags: self.resource_tags.clone().map(into_tag_map),
            instance_metadata_options: self.instance_metadata_options.clone(),
            tags: self.tags.clone().map(into_tag_map),
            placement: self.placement.clone(),
            terminate_instance_on_completion: Some(self.terminate_instance_on_completion),
            parent_instance: self.parent_instance.clone(),
        }
    }
}

/// What a successful `CreateInfrastructureConfiguration` filed about the request it
/// served, so a retry of that request can be told from a different one carrying the
/// same `clientToken`.
#[derive(Debug, Clone)]
pub struct CreateInfrastructureConfigurationReceipt {
    /// The creation parameters the original request carried
    /// ([`create_request_fingerprint`]).
    pub fingerprint: String,
    /// The `requestId` the original response named.
    pub request_id: String,
    /// The `clientToken` the original response echoed.
    pub client_token: String,
    /// The ARN the original response named.
    pub arn: String,
    /// Which receipt this is, counted from one, so the OLDEST is the one the index
    /// evicts once it is full.
    pub filed: u64,
}

/// Per-service state for com-amazonaws-imagebuilder-imagebuilder. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsImagebuilderImagebuilderState {
    /// Every stored infrastructure configuration, keyed by its own ARN — which
    /// carries both its Region and its normalized name, so one key says where a
    /// configuration lives and which name is taken there.
    pub infrastructure_configurations: BTreeMap<String, InfrastructureConfigurationRecord>,
    /// The answer each `clientToken` already got, keyed by (Region, token). Bounded
    /// on its own, by age, at [`MAX_CREATE_RECEIPTS`] — see
    /// [`Self::insert_infrastructure_configuration`].
    pub infrastructure_configuration_receipts:
        BTreeMap<(String, String), CreateInfrastructureConfigurationReceipt>,
    /// How many receipts have been filed, which is what orders them by age.
    pub infrastructure_configuration_receipts_filed: u64,
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The configuration `arn` names, but only when it belongs to `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a configuration
    /// of another Region answers as if it did not exist.
    #[must_use]
    pub fn infrastructure_configuration(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<&InfrastructureConfigurationRecord> {
        self.infrastructure_configurations
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created configuration together with the receipt for the request
    /// that created it.
    ///
    /// The receipt index is bounded by AGE rather than by refusing the create: a
    /// receipt is only ever read by a caller still retrying its own request, so past
    /// [`MAX_CREATE_RECEIPTS`] — the index's own bound — the oldest is dropped. A
    /// retry whose receipt was evicted is then answered as a fresh request — which
    /// the name-uniqueness rule refuses, the same answer any caller asking for a name
    /// already taken gets — and the index can never wedge a create.
    pub fn insert_infrastructure_configuration(
        &mut self,
        record: InfrastructureConfigurationRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.infrastructure_configuration_receipts_filed = self
            .infrastructure_configuration_receipts_filed
            .saturating_add(1);
        let receipt = CreateInfrastructureConfigurationReceipt {
            fingerprint,
            request_id,
            client_token: client_token.to_string(),
            arn: record.arn.clone(),
            filed: self.infrastructure_configuration_receipts_filed,
        };
        while self.infrastructure_configuration_receipts.len() >= MAX_CREATE_RECEIPTS {
            let oldest = self
                .infrastructure_configuration_receipts
                .iter()
                .min_by_key(|(_, filed)| filed.filed)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.infrastructure_configuration_receipts.remove(&key);
                }
                None => break,
            }
        }
        self.infrastructure_configuration_receipts
            .insert((record.region.clone(), client_token.to_string()), receipt);
        self.infrastructure_configurations
            .insert(record.arn.clone(), record);
    }

    /// The receipt `client_token` already earned in `request_region`, if any.
    #[must_use]
    pub fn infrastructure_configuration_receipt(
        &self,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateInfrastructureConfigurationReceipt> {
        self.infrastructure_configuration_receipts
            .get(&(request_region.to_string(), client_token.to_string()))
    }

    /// Whether the store is already holding as many configurations as this emulator
    /// will keep ([`MAX_INFRASTRUCTURE_CONFIGURATIONS`]).
    #[must_use]
    pub fn infrastructure_configuration_store_is_full(&self) -> bool {
        self.infrastructure_configurations.len() >= MAX_INFRASTRUCTURE_CONFIGURATIONS
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsImagebuilderImagebuilderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsImagebuilderImagebuilderState>)
}

/// The normalized form of an infrastructure configuration name: the form its ARN
/// carries, and the form two names are compared in.
///
/// From the modelled `name` member: "Image Builder generates the infrastructure
/// configuration ARN from a normalized form of the name, so names that differ only in
/// case, spaces, or underscores count as the same name." Case folds down, and a space
/// and an underscore are the same separator as a hyphen — which is what makes those
/// three forms one name. The modelled `ResourceName` pattern admits only
/// `[-_A-Za-z0-9 ]`, so the result is always `[a-z0-9-]+` and therefore inside the
/// `[a-z0-9-_]+` the ARN pattern allows.
#[must_use]
pub fn normalized_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// The ARN of the infrastructure configuration `name` names in `region`.
#[must_use]
pub fn infrastructure_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX}{}",
            normalized_name(name)
        ),
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only a non-colon field there, so anything carrying a colon
/// or anything implausibly long is refused here and the default is used instead: a
/// generated ARN can never carry text the model does not admit. A request whose scope
/// is unusable is therefore served by the DEFAULT Region, so the configuration it
/// creates lives there and is read back through that Region's ARN — the same place a
/// request carrying no scope at all is served.
#[must_use]
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

/// Render `at` the way this model's string `DateTime` members carry an instant
/// (ISO 8601, e.g. `2026-10-24T01:51:48Z`).
#[must_use]
pub fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Mint the `requestId` one response names the request it answers.
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes. The VALUE is per-call
/// and is not a requirement: nothing can predict it and no caller may depend on a
/// particular one. What it must do is name ONE response, so it is drawn from a
/// per-process counter run through [`scramble`] — a bijection, so two responses never
/// carry the same id — mixed with the instant the response was answered, so two runs
/// of this emulator do not mint the same sequence.
#[must_use]
pub fn new_request_id() -> String {
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let instant = since_epoch
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(since_epoch.subsec_nanos()));
    // The high half is a bijection of the counter alone, which is what makes two ids
    // minted in the same nanosecond differ; the low half carries the instant.
    let high = scramble(counter);
    let low = scramble(instant ^ scramble(counter.wrapping_add(1)));
    let hex = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Mix a counter so consecutive values render as unrelated tokens.
///
/// SplitMix64's finaliser applied to the counter times the 64-bit golden ratio. Every
/// step (multiply by an odd constant, xor-shift) is a bijection on `u64`, so the whole
/// function is one: distinct counters never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A caller-submitted tag map, ordered so a read reports it the same way twice.
#[must_use]
pub fn into_ordered_tags(tags: HashMap<String, String>) -> BTreeMap<String, String> {
    tags.into_iter().collect()
}

/// An ordered tag map as the modelled map member carries it.
fn into_tag_map(tags: BTreeMap<String, String>) -> HashMap<String, String> {
    tags.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Idempotency
//
// `clientToken` is an @idempotencyToken: "If you retry a request with the same client
// token, Image Builder returns the original response without running the operation
// again", and a token reused with different parameter values is the modelled
// `IdempotentParameterMismatchException`. Telling those two apart needs the ORIGINAL
// request's creation parameters, so a successful create files their rendering and a
// later request carrying the same token is rendered the same way and compared.
//
// The rendering is a total, deterministic text over every creation parameter: each
// member is length-prefixed so no two different inputs can render alike, and every map
// is written in key order so one request cannot render two ways.
// ---------------------------------------------------------------------------

/// How an absent member renders inside a fingerprint.
const ABSENT: &str = "-";

/// A deterministic rendering of the creation parameters one
/// `CreateInfrastructureConfiguration` request carries.
///
/// `clientToken` and `dryRun` are deliberately NOT part of it: the token is the key
/// this rendering is compared under, and a dry run never files a receipt.
#[must_use]
pub fn create_request_fingerprint(input: &CreateInfrastructureConfigurationInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert(
        "instanceProfileName",
        text(Some(input.instance_profile_name.as_str())),
    );
    fields.insert("subnetId", text(input.subnet_id.as_deref()));
    fields.insert("keyPair", text(input.key_pair.as_deref()));
    fields.insert("snsTopicArn", text(input.sns_topic_arn.as_deref()));
    fields.insert("instanceTypes", list(input.instance_types.as_deref()));
    fields.insert("securityGroupIds", list(input.security_group_ids.as_deref()));
    fields.insert("resourceTags", tag_map(input.resource_tags.as_ref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert(
        "terminateInstanceOnFailure",
        flag(input.terminate_instance_on_failure),
    );
    fields.insert(
        "terminateInstanceOnCompletion",
        flag(input.terminate_instance_on_completion),
    );
    fields.insert(
        "logging",
        input.logging.as_ref().map_or_else(
            || ABSENT.to_string(),
            |logging| {
                logging.s3_logs.as_ref().map_or_else(
                    || ABSENT.to_string(),
                    |s3| {
                        group(&[
                            text(s3.s3_bucket_name.as_deref()),
                            text(s3.s3_key_prefix.as_deref()),
                        ])
                    },
                )
            },
        ),
    );
    fields.insert(
        "instanceMetadataOptions",
        input.instance_metadata_options.as_ref().map_or_else(
            || ABSENT.to_string(),
            |options| {
                group(&[
                    text(options.http_tokens.as_deref()),
                    number(options.http_put_response_hop_limit),
                ])
            },
        ),
    );
    fields.insert(
        "placement",
        input.placement.as_ref().map_or_else(
            || ABSENT.to_string(),
            |placement| {
                group(&[
                    text(placement.availability_zone.as_deref()),
                    text(placement.tenancy.as_deref()),
                    text(placement.host_id.as_deref()),
                    text(placement.host_resource_group_arn.as_deref()),
                ])
            },
        ),
    );
    fields.insert(
        "parentInstance",
        input.parent_instance.as_ref().map_or_else(
            || ABSENT.to_string(),
            |parent| {
                group(&[
                    text(Some(parent.auto_scaling_group.name.as_str())),
                    number(parent.auto_scaling_group.capacity_wait_timeout_seconds),
                ])
            },
        ),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// One string member, length-prefixed so no two different values render alike.
fn text(value: Option<&str>) -> String {
    value.map_or_else(
        || ABSENT.to_string(),
        |value| format!("{}:{value}", value.len()),
    )
}

/// One optional boolean member.
fn flag(value: Option<bool>) -> String {
    value.map_or_else(|| ABSENT.to_string(), |value| value.to_string())
}

/// One optional integer member.
fn number(value: Option<i32>) -> String {
    value.map_or_else(|| ABSENT.to_string(), |value| value.to_string())
}

/// One list member, in the order the request carried it — a list's order is part of
/// its value, so two orders are two different requests.
fn list(values: Option<&[String]>) -> String {
    values.map_or_else(
        || ABSENT.to_string(),
        |values| {
            group(
                &values
                    .iter()
                    .map(|value| text(Some(value)))
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// One map member, in key order — a map carries no order of its own, so the rendering
/// imposes one and two orderings of the same pairs render alike.
fn tag_map(values: Option<&HashMap<String, String>>) -> String {
    values.map_or_else(
        || ABSENT.to_string(),
        |values| {
            let ordered: BTreeMap<&String, &String> = values.iter().collect();
            group(
                &ordered
                    .into_iter()
                    .map(|(key, value)| {
                        format!("{}={}", text(Some(key)), text(Some(value)))
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// Several already-rendered parts as one member's value.
fn group(parts: &[String]) -> String {
    format!("[{}]", parts.join(","))
}

// ---------------------------------------------------------------------------
// How this service words a refusal
//
// Each wording lives here once, so two operations state the same failure the same way.
// ---------------------------------------------------------------------------

/// Why a request naming a subnet but no security group is refused.
///
/// The rule is the `subnetId` member's own: "If you specify `subnetId`, you must also
/// specify one or more security group IDs in `securityGroupIds`. Otherwise, the
/// request fails." The wording names the two members and the requirement, and
/// withholds the values the request carried.
#[must_use]
pub fn subnet_without_security_group_message() -> String {
    "The request must specify one or more securityGroupIds when it specifies a subnetId."
        .to_string()
}

/// Why a request assigning a reserved metadata-tag key is refused.
#[must_use]
pub fn reserved_resource_tag_key_message(key: &str) -> String {
    format!("The resourceTags key {key} is reserved and can't be assigned.")
}

/// Why a create whose name is already taken in this Region is refused.
#[must_use]
pub fn infrastructure_configuration_exists_message(name: &str) -> String {
    format!(
        "An infrastructure configuration named {name} already exists in this Amazon Web Services \
         Region."
    )
}

/// Why a read naming no stored configuration reports nothing.
///
/// Names the RESOURCE TYPE that is absent and the ARN the caller asked about, and
/// nothing about any configuration that does exist.
#[must_use]
pub fn infrastructure_configuration_not_found_message(arn: &str) -> String {
    format!("The infrastructure configuration {arn} does not exist.")
}

/// Why a create was not performed because the request asked only for validation.
///
/// From the `dryRun` member's own documentation: "Validates the required permissions
/// and request parameters without performing the operation. If validation succeeds,
/// the operation returns a DryRunOperationException error response."
#[must_use]
pub fn dry_run_message() -> String {
    "Request would have succeeded, but DryRun flag is set.".to_string()
}

/// Why the same `clientToken` carrying different creation parameters is refused.
#[must_use]
pub fn idempotent_parameter_mismatch_message() -> String {
    "The clientToken was already used for a request with different parameter values.".to_string()
}

/// Why a create is refused once the store is full.
#[must_use]
pub fn store_full_message() -> String {
    format!(
        "This account already holds the maximum of {MAX_INFRASTRUCTURE_CONFIGURATIONS} \
         infrastructure configurations."
    )
}

/// Why an operation could not reach this service's store at all.
#[must_use]
pub fn store_unavailable_message() -> String {
    "The infrastructure configuration store is unavailable.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_differing_only_in_case_spaces_or_underscores_normalize_alike() {
        let cases: &[(&str, &str)] = &[
            ("Build-Infra", "build-infra"),
            ("build infra", "build-infra"),
            ("build_infra", "build-infra"),
            ("BUILD INFRA", "build-infra"),
            ("build-infra", "build-infra"),
            ("Prod_Linux Build-01", "prod-linux-build-01"),
        ];
        for (name, expected) in cases {
            let actual = normalized_name(name);
            assert_eq!(
                &actual, expected,
                "case {name:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_minted_request_id_is_uuid_shaped_and_fresh() {
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..16 {
            let id = new_request_id();
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                groups,
                vec![8, 4, 4, 4, 12],
                "case new_request_id: expected 8-4-4-4-12 groups, got {id:?}"
            );
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "case new_request_id: expected lowercase hex, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case new_request_id: expected a fresh id, got the already-minted {id:?}"
            );
            seen.push(id);
        }
    }
}
