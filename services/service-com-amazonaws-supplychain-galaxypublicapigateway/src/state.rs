// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supplychain-galaxypublicapigateway service.
//!
//! The AWS Supply Chain **Instance** is this service's primary resource: every other
//! resource (data-lake namespaces and datasets, data-integration flows and events) is
//! addressed by an `instanceId`, so the instance store lives here and the per-operation
//! handlers in `src/ops/` read and write it through [`service_state`].

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DataLakeDataset, DataLakeDatasetPartitionSpec, DataLakeDatasetSchema, Instance,
};

/// The sentence AWS Supply Chain refuses the Product Bill Of Materials (BOM) import API
/// with: that API is WITHDRAWN, so both of its operations
/// (`CreateBillOfMaterialsImportJob` and `GetBillOfMaterialsImportJob`) answer the
/// modelled `AccessDeniedException` (HTTP 403) carrying it.
///
/// Its SOURCE is the recorded wire response of this API — the recorded unit
/// `supplychain.bom-import-job-fails-without-s3-access` probes both halves of it and both
/// answer this sentence. The model declares no deprecation and fixes no text, and the
/// operations' own documentation still describes creating and reading an import job, so a
/// later reader re-verifies this against a recording rather than against the model. The
/// whole sentence is FIXED: it names the API that has been withdrawn and the channel to
/// ask about it, and carries nothing from the request, so no part of it is interpolated.
/// Stated once, here, so the two operations cannot drift into two sentences for one
/// refusal.
pub const WITHDRAWN_BOM_IMPORT_API_MESSAGE: &str = "This API has been deprecated. Please contact \
                                                    ASC support team at \
                                                    https://aws.amazon.com/contact-us/ for more \
                                                    details.";

/// `InstanceState` as the model spells it: a freshly created instance is still being
/// built. `CreateInstance`'s own documentation — "Upon receiving a CreateInstance
/// request, Amazon Web Services Supply Chain immediately returns the instance resource,
/// instance ID, and the initializing state" — is what pins this as the initial state.
pub const INSTANCE_STATE_INITIALIZING: &str = "Initializing";

/// `InstanceState` for an instance whose creation finished: the terminal healthy state
/// a client polling `GetInstance` for readiness waits for.
pub const INSTANCE_STATE_ACTIVE: &str = "Active";

/// `InstanceState` for an instance whose deletion has been accepted.
pub const INSTANCE_STATE_DELETING: &str = "Deleting";

/// How long after `CreateInstance` the clock owes an instance its
/// `Initializing` -> `Active` transition.
///
/// CHOSEN, not documented: AWS states only that `CreateInstance` is asynchronous and
/// returns `Initializing`, never how long initialization takes. The span is deliberately
/// non-zero so a caller that reads the instance back immediately still observes
/// `Initializing` (and so a readiness poll has something to converge on), and short
/// enough that the poll terminates promptly.
pub const INSTANCE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The first `versionNumber` a stored instance carries; every change bumps it by one.
pub const INSTANCE_INITIAL_VERSION: f64 = 1.0;

/// The number of instances this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// `CreateInstance` is the only thing that adds one, so the store needs a maximum entry
/// COUNT: neither a TTL nor an eviction policy is available to it, because `GetInstance`
/// must still answer what `CreateInstance` created. Each record's size is already bounded
/// by the modelled `@length` constraints on its members, so the count is the remaining
/// growth term, and a create past it is refused with the service's own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — which `CreateInstance` declares and no
/// read does. The number is the emulator's own bound, not a published AWS value: AWS
/// quotas instances per account as an ADJUSTABLE limit, so no single number is the
/// contract, and this one is far above any ordinary workflow.
pub const MAX_INSTANCES: usize = 100;

/// The number of `clientToken` entries the idempotency table holds.
///
/// Each token names the one instance it created and `DeleteInstance` drops the tokens
/// naming a record it removes, so the table never holds more entries than there are live
/// instances — but the keys are caller-supplied strings, so the bound is enforced at the
/// one growth site rather than left to that invariant. Same cap, same refusal as
/// [`MAX_INSTANCES`].
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_INSTANCES;

/// How many v4 UUIDs [`ComAmazonawsSupplychainGalaxypublicapigatewayState::fresh_instance_id`]
/// draws before giving up.
///
/// A bound rather than a retry loop: with at most [`MAX_INSTANCES`] live ids, a single
/// draw colliding is already vanishingly unlikely, so exhausting these attempts means
/// something is wrong with the generator — which the caller reports as a server error
/// instead of spinning while holding the state lock.
pub const ID_MINT_ATTEMPTS: usize = 8;

/// One persisted AWS Supply Chain instance — the whole record, not the wire shape.
/// [`InstanceRecord::to_wire`] projects it onto the modelled `Instance` structure, so
/// every handler answers from exactly these stored values.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub instance_id: String,
    pub aws_account_id: String,
    pub state: String,
    pub error_message: Option<String>,
    pub instance_name: Option<String>,
    pub instance_description: Option<String>,
    pub kms_key_arn: Option<String>,
    pub web_app_dns_domain: Option<String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub version_number: f64,
    pub tags: BTreeMap<String, String>,
    /// The instant at which this instance's `Initializing` -> `Active` transition
    /// becomes due. Stored rather than slept on: the transition is materialised by
    /// whichever call next observes the record (see
    /// [`ComAmazonawsSupplychainGalaxypublicapigatewayState::materialize_due_transitions`]).
    pub active_due: SystemTime,
}

impl InstanceRecord {
    /// A brand-new instance in `Initializing`, owned by the emulator's account.
    ///
    /// The optional members (`instanceName`, `instanceDescription`, `kmsKeyArn`,
    /// `webAppDnsDomain`, `tags`) are left absent for the caller to fill from the
    /// request, so a member the request omitted stays absent rather than being
    /// invented.
    pub fn new(instance_id: String, now: SystemTime) -> Self {
        Self {
            instance_id,
            aws_account_id: ::engine::emulator_account_id().to_string(),
            state: INSTANCE_STATE_INITIALIZING.to_string(),
            error_message: None,
            instance_name: None,
            instance_description: None,
            kms_key_arn: None,
            web_app_dns_domain: None,
            created_time: now,
            last_modified_time: now,
            version_number: INSTANCE_INITIAL_VERSION,
            tags: BTreeMap::new(),
            active_due: now + INSTANCE_ACTIVATION_DELAY,
        }
    }

    /// This record as the modelled `Instance` shape every instance operation returns.
    pub fn to_wire(&self) -> Instance {
        Instance {
            instance_id: self.instance_id.clone(),
            aws_account_id: self.aws_account_id.clone(),
            state: self.state.clone(),
            error_message: self.error_message.clone(),
            web_app_dns_domain: self.web_app_dns_domain.clone(),
            created_time: Some(as_date_time(self.created_time)),
            last_modified_time: Some(as_date_time(self.last_modified_time)),
            instance_name: self.instance_name.clone(),
            instance_description: self.instance_description.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            version_number: Some(self.version_number),
        }
    }

    /// Record a change made at `now`: a mutation bumps both `lastModifiedTime` and
    /// `versionNumber`, which is what lets a client detect it read a stale copy.
    pub fn touch(&mut self, now: SystemTime) {
        self.last_modified_time = now;
        self.version_number += 1.0;
    }
}

/// A `SystemTime` as the modelled timestamp type.
fn as_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(time)
}

/// The ARN namespace every AWS Supply Chain resource carries: `AscResourceArn`'s
/// modelled pattern fixes the prefix `arn:aws:scn`.
pub const ARN_SERVICE: &str = "scn";

/// The region a request that carries no SigV4 credential scope is served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region the caller addressed, read from the request's SigV4 credential scope, so a
/// resource's ARN names the region the call that created it was signed for. Falls back to
/// [`DEFAULT_REGION`] for a request with no scope to read.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The ARN of one data-lake dataset.
///
/// `AscResourceArn` (the shape `DataLakeDataset::arn` targets) fixes the form: the
/// partition and service are literal, the region and account are the instance ARN's, and
/// everything after the instance id is free `[-_./A-Za-z0-9]*` — so the dataset's
/// namespace and name extend the instance's resource portion along the same path segments
/// the operation's URI uses (`.../namespaces/{namespace}/datasets/{name}`).
pub fn dataset_arn(region: &str, instance_id: &str, namespace: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("instance/{instance_id}/namespaces/{namespace}/datasets/{name}"),
    )
}

/// The data-lake namespaces an instance has without anybody creating them.
///
/// SOURCE: the `namespace` member's own documentation on the dataset operations — "besides
/// the custom defined namespace, every instance comes with below pre-defined namespaces:
/// asc […] default […]". Every other namespace exists only once it has been created, so a
/// dataset addressed to one that was never created names no namespace at all.
pub const PREDEFINED_NAMESPACES: [&str; 2] = ["asc", "default"];

/// Whether `namespace` is one an instance comes with ([`PREDEFINED_NAMESPACES`]).
pub fn is_predefined_namespace(namespace: &str) -> bool {
    PREDEFINED_NAMESPACES.contains(&namespace)
}

/// The number of datasets one instance holds, across all of its namespaces.
///
/// Same reasoning as [`MAX_INSTANCES`]: the store keeps its records for the life of the
/// process, each record's size is already bounded by the modelled `@length` constraints
/// on its members, so the entry COUNT is the remaining growth term and it is capped at
/// the one site that adds a dataset — refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) that `CreateDataLakeDataset` declares and
/// no read does. The number is the emulator's own bound, not a published AWS value: AWS
/// quotas datasets per instance as an adjustable limit, so no single number is the
/// contract, and this one is far above any ordinary workflow.
pub const MAX_DATASETS_PER_INSTANCE: usize = 200;

/// How a data-lake dataset is addressed: `(instanceId, namespace, name)`, the three
/// `@httpLabel` members every dataset operation's URI carries, in that order.
///
/// A `BTreeMap` keyed by it therefore groups one instance's namespaces, and one
/// namespace's datasets, contiguously and in name order — the order
/// `ListDataLakeDatasets` pages through.
pub type DatasetKey = (String, String, String);

/// The [`DatasetKey`] for one dataset, so every handler builds the key the same way.
pub fn dataset_key(instance_id: &str, namespace: &str, name: &str) -> DatasetKey {
    (
        instance_id.to_string(),
        namespace.to_string(),
        name.to_string(),
    )
}

/// One persisted data-lake dataset — the whole record, not the wire shape.
/// [`DataLakeDatasetRecord::to_wire`] projects it onto the modelled `DataLakeDataset`
/// structure, so every handler answers from exactly these stored values.
#[derive(Debug, Clone)]
pub struct DataLakeDatasetRecord {
    pub instance_id: String,
    pub namespace: String,
    pub name: String,
    pub arn: String,
    pub schema: DataLakeDatasetSchema,
    pub description: Option<String>,
    pub partition_spec: Option<DataLakeDatasetPartitionSpec>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The dataset's tags. `tags` is `notProperty` on the request and absent from the
    /// `DataLakeDataset` shape, so they are stored for the tagging operations to report
    /// rather than projected onto the wire shape here.
    pub tags: BTreeMap<String, String>,
}

impl DataLakeDatasetRecord {
    /// This record as the modelled `DataLakeDataset` shape every dataset operation
    /// returns.
    pub fn to_wire(&self) -> DataLakeDataset {
        DataLakeDataset {
            instance_id: self.instance_id.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            schema: self.schema.clone(),
            description: self.description.clone(),
            partition_spec: self.partition_spec.clone(),
            created_time: as_date_time(self.created_time),
            last_modified_time: as_date_time(self.last_modified_time),
        }
    }
}

/// Per-service state for com-amazonaws-supplychain-galaxypublicapigateway. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSupplychainGalaxypublicapigatewayState {
    /// Every live instance, keyed by `instanceId`. A `BTreeMap` so `ListInstances`
    /// pages in one stable order and a page token can name the key to resume at.
    pub instances: BTreeMap<String, InstanceRecord>,
    /// `clientToken` -> `instanceId`, so a replayed `CreateInstance` (the operation is
    /// `@idempotent` and its `clientToken` is an `@idempotencyToken`) returns the
    /// instance the first call created instead of creating a second one.
    pub idempotency: HashMap<String, String>,
    /// Every live data-lake dataset, keyed by [`DatasetKey`]. A `BTreeMap` so an
    /// instance's datasets are contiguous and in namespace-then-name order.
    pub datasets: BTreeMap<DatasetKey, DataLakeDatasetRecord>,
}

impl ComAmazonawsSupplychainGalaxypublicapigatewayState {
    /// Materialise every state transition the clock already owes at `now`.
    ///
    /// Called at the top of each instance operation — including the reporting ones —
    /// because the `Initializing` -> `Active` transition was owed at `now` whether or
    /// not anybody called: it is the clock's work, not the reader's. Nothing else about
    /// a record changes here, so repeated reads stay identical once the transition is
    /// done.
    ///
    /// The scan runs under the state lock and touches every stored record, so its cost
    /// is bounded by [`MAX_INSTANCES`] — the cap `CreateInstance` enforces — and by
    /// nothing a caller controls beyond it.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        for record in self.instances.values_mut() {
            if record.state == INSTANCE_STATE_INITIALIZING && record.active_due <= now {
                record.state = INSTANCE_STATE_ACTIVE.to_string();
            }
        }
    }

    /// Whether a further `CreateInstance` would push either caller-driven store past its
    /// cap ([`MAX_INSTANCES`] / [`MAX_IDEMPOTENCY_TOKENS`]).
    ///
    /// Checked at that one growth site, before anything is inserted; a replay of a token
    /// already in the table inserts nothing and so is answered even at the cap.
    pub fn at_capacity(&self) -> bool {
        self.instances.len() >= MAX_INSTANCES || self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS
    }

    /// A fresh `instanceId`: a lowercase-hex v4 UUID, as the model's `UUID` shape
    /// requires (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`),
    /// that no live instance already holds.
    ///
    /// `None` when [`ID_MINT_ATTEMPTS`] draws collided — which cannot happen in
    /// practice, so the caller answers it with a modelled server error rather than
    /// spinning under the state lock.
    pub fn fresh_instance_id(&self) -> Option<String> {
        (0..ID_MINT_ATTEMPTS)
            .map(|_| new_uuid_v4())
            .find(|candidate| !self.instances.contains_key(candidate))
    }

    /// How many datasets `instance_id` holds, across all of its namespaces — what
    /// [`MAX_DATASETS_PER_INSTANCE`] caps.
    pub fn dataset_count(&self, instance_id: &str) -> usize {
        self.datasets
            .keys()
            .filter(|(instance, _, _)| instance == instance_id)
            .count()
    }

    /// Drop every dataset of `instance_id`: a dataset is addressed by its instance, so
    /// none of them outlives the instance whose deletion removed it.
    pub fn drop_instance_datasets(&mut self, instance_id: &str) {
        self.datasets
            .retain(|(instance, _, _), _| instance != instance_id);
    }
}

/// A fresh random (version 4) UUID in its canonical lowercase hyphenated form.
///
/// Drawn through `emulator_telemetry`'s own v4 generator, a crate this service already
/// depends on, so minting an id adds no dependency — the same seam
/// `services/service-com-amazonaws-signin-signin/src/state.rs` draws its opaque values
/// through.
fn new_uuid_v4() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// Whether `value` is an `instanceId` of the model's `UUID` shape: length 36 and
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
///
/// Stated once, here, so the handler that validates a resume key and the tests that
/// pin the shape of a minted id cannot drift apart. `Uuid::parse_str` is deliberately
/// NOT the check: it also accepts the braced, urn and simple forms and uppercase hex,
/// none of which the model's pattern allows.
pub fn is_modelled_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    value.len() == 36
        && groups.len() == GROUPS.len()
        && groups.iter().zip(GROUPS).all(|(group, width)| {
            group.len() == width
                && group
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSupplychainGalaxypublicapigatewayState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSupplychainGalaxypublicapigatewayState>)
}
