// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshiftserverless-redshiftserverless service.
//!
//! The namespace + workgroup store, plus the little that every handler of that resource
//! pair shares: the server-derived fields (id, ARN, endpoint address, creation date), the
//! clock-owed status transition a just-created or just-modified workgroup settles
//! through, and the opaque page token the two paginated lists hand out.
//!
//! `src/ops/mod.rs` is generated, so a helper more than one operation reaches lives here
//! rather than in a second module under `src/ops/`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{Namespace, Tag, Workgroup};

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`). Also its SigV4
/// signing name and the host label its endpoints carry.
pub const ARN_NAMESPACE: &str = "redshift-serverless";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential scope
/// names one. `engine::emulator_arn`'s own doc fixes this default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `CreateNamespace`'s `dbName` default — `Namespace$dbName` is "the name of the first
/// database created in the namespace", and Redshift's first database is `dev`.
pub const DEFAULT_DB_NAME: &str = "dev";

/// `CreateWorkgroup`'s `baseCapacity` default, in Redshift Processing Units (RPUs).
pub const DEFAULT_BASE_CAPACITY: i32 = 128;

/// `CreateWorkgroup`'s `port` default. From the model's `Workgroup$port` doc: "Valid port
/// ranges are 5431-5455 and 8191-8215. The default is 5439."
pub const DEFAULT_PORT: i32 = 5439;

/// The port ranges a workgroup accepts, from the same `Workgroup$port` doc sentence as
/// [`DEFAULT_PORT`]: 5431-5455 and 8191-8215, both inclusive.
pub const PORT_RANGES: [(i32, i32); 2] = [(5431, 5455), (8191, 8215)];

/// The single Redshift Serverless version this emulator serves, reported as a workgroup's
/// `workgroupVersion`. One emulator build is one release, so the value is a property of
/// the build and not of any request.
pub const WORKGROUP_VERSION: &str = "1.0";

/// The page bound a list applies when the caller names none: `maxResults`'
/// `@range(min: 1, max: 100)` on `ListNamespaces` / `ListWorkgroups`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The account's quota for each resource in one Region, from AWS's "Quotas for Amazon
/// Redshift Serverless": 25 namespaces and 25 workgroups per account per Region (both
/// adjustable on request, so this is the DEFAULT quota). It bounds the store as well as
/// the API: a caller cannot accumulate records past it.
pub const MAX_NAMESPACES: usize = 25;
/// See [`MAX_NAMESPACES`].
pub const MAX_WORKGROUPS: usize = 25;

/// The `WorkgroupStatus` values this slice reports
/// (`smithy_query(query="shape", shape="WorkgroupStatus")` -> CREATING, AVAILABLE,
/// MODIFYING, DELETING).
pub const WORKGROUP_CREATING: &str = "CREATING";
/// See [`WORKGROUP_CREATING`].
pub const WORKGROUP_AVAILABLE: &str = "AVAILABLE";
/// See [`WORKGROUP_CREATING`].
pub const WORKGROUP_MODIFYING: &str = "MODIFYING";
/// See [`WORKGROUP_CREATING`].
pub const WORKGROUP_DELETING: &str = "DELETING";

/// The `NamespaceStatus` values this slice reports
/// (`smithy_query(query="shape", shape="NamespaceStatus")` -> AVAILABLE, MODIFYING,
/// DELETING). There is no `CREATING`: a namespace holds no compute, so it is available as
/// soon as it exists and has no transition to settle.
pub const NAMESPACE_AVAILABLE: &str = "AVAILABLE";
/// See [`NAMESPACE_AVAILABLE`].
pub const NAMESPACE_DELETING: &str = "DELETING";

/// How long after the call that starts it a workgroup's status transition falls due. A
/// span this emulator chooses, not a duration the service documents: it only has to be
/// long enough that a caller acting straight after the create still observes the
/// transient status, and short enough to poll through.
const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// True when `port` lies in one of the ranges `Workgroup$port` documents.
pub fn port_in_documented_range(port: i32) -> bool {
    PORT_RANGES
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&port))
}

/// `arn:aws:redshift-serverless:<region>:<account>:namespace/<namespaceId>`.
pub fn namespace_arn(region: &str, namespace_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("namespace/{namespace_id}"))
}

/// `arn:aws:redshift-serverless:<region>:<account>:workgroup/<workgroupId>`.
pub fn workgroup_arn(region: &str, workgroup_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("workgroup/{workgroup_id}"))
}

/// The DNS address of the endpoint a workgroup serves:
/// `<workgroupName>.<account>.<region>.redshift-serverless.amazonaws.com`.
pub fn endpoint_address(workgroup_name: &str, region: &str) -> String {
    format!(
        "{workgroup_name}.{}.{region}.{ARN_NAMESPACE}.amazonaws.com",
        ::engine::emulator_account_id()
    )
}

/// A server-minted resource id (`namespaceId` / `workgroupId`): a v4 UUID, the form AWS
/// mints them in. Random per call, so it is stored and then read back, never recomputed.
pub fn new_resource_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Now, as the `date-time` timestamp `Namespace$creationDate` / `Workgroup$creationDate`
/// are modelled with.
pub fn now_timestamp() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(std::time::SystemTime::now())
}

/// The region this request is signed for, which is the region its resources' ARNs carry.
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A status change the clock owes a workgroup.
///
/// `WorkgroupStatus` models `CREATING` and `MODIFYING` beside `AVAILABLE`, so a workgroup
/// is reported in one of them while the compute it describes is being brought up or
/// changed, and reaches `AVAILABLE` on its own a moment later with nobody calling. The
/// due instant is STORED and applied when a later call observes the record; no handler
/// ever sleeps for it.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The instant from which [`status`](Self::status) is the workgroup's real status.
    pub due_at: Instant,
    /// The status the workgroup settles into.
    pub status: &'static str,
}

/// A stored namespace.
#[derive(Debug, Clone)]
pub struct NamespaceRecord {
    /// The resource every reader of this namespace returns, held whole so a get, a list
    /// and a delete all answer exactly what the create stored.
    pub namespace: Namespace,
    /// Held because the service holds it, and never returned: `Namespace` models no
    /// password member, so no read can echo it.
    pub admin_user_password: Option<String>,
    /// Whether the caller asked the service to manage the admin password itself.
    pub manage_admin_password: bool,
    /// The tags the create request carried. `Namespace` models no `tags` member, so they
    /// are reached through the tagging operations rather than echoed on a read.
    pub tags: Vec<Tag>,
}

/// A stored workgroup.
#[derive(Debug, Clone)]
pub struct WorkgroupRecord {
    /// The resource every reader of this workgroup returns, held whole so a get, a list,
    /// an update and a delete all answer exactly what the last write stored.
    pub workgroup: Workgroup,
    /// The tags the create request carried; see [`NamespaceRecord::tags`].
    pub tags: Vec<Tag>,
    /// The transition the clock owes this workgroup, while one is outstanding.
    pub pending_status: Option<PendingStatus>,
}

impl WorkgroupRecord {
    /// Report `transient` from now, and owe `settled` from [`SETTLE_AFTER`] onwards.
    pub fn begin_transition(&mut self, transient: &str, settled: &'static str) {
        self.workgroup.status = Some(transient.to_string());
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + SETTLE_AFTER,
            status: settled,
        });
    }

    /// Materialise the transition the clock has already made due, if any.
    ///
    /// This is the one write a reporting operation may make: the change was owed at `now`
    /// whether or not anybody called, so applying it does not change what a later caller
    /// sees — it is what a later caller sees.
    pub fn settle(&mut self, now: Instant) {
        if let Some(pending) = self.pending_status.take_if(|pending| now >= pending.due_at) {
            self.workgroup.status = Some(pending.status.to_string());
        }
    }
}

/// Per-service state for com-amazonaws-redshiftserverless-redshiftserverless. A handler reaches it through [`service_state`].
///
/// Both resources are keyed by NAME, which is their identity in this API (every operation
/// addresses them by `namespaceName` / `workgroupName`), and held in a `BTreeMap` so the
/// two paginated lists walk them in one deterministic order that a page token can resume.
#[derive(Debug, Default)]
pub struct ComAmazonawsRedshiftserverlessRedshiftserverlessState {
    /// Namespaces by `namespaceName`.
    pub namespaces: BTreeMap<String, NamespaceRecord>,
    /// Workgroups by `workgroupName`.
    pub workgroups: BTreeMap<String, WorkgroupRecord>,
}

impl ComAmazonawsRedshiftserverlessRedshiftserverlessState {
    /// Apply every workgroup status transition the clock has already made due. Call it
    /// before reading or writing a workgroup, so no caller observes a status the service
    /// has already moved on from. See [`WorkgroupRecord::settle`].
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        for record in self.workgroups.values_mut() {
            record.settle(now);
        }
    }

    /// The names of the workgroups attached to `namespace_name`, in list order. A
    /// namespace with any of these cannot be deleted.
    pub fn workgroups_in_namespace(&self, namespace_name: &str) -> Vec<&str> {
        self.workgroups
            .values()
            .filter(|record| {
                record.workgroup.namespace_name.as_deref() == Some(namespace_name)
            })
            .filter_map(|record| record.workgroup.workgroup_name.as_deref())
            .collect()
    }
}

/// One page of a listing: the records it carries, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a, V> {
    /// The records of this page, in key order.
    pub items: Vec<&'a V>,
    /// Set only when at least one more record remains after this page.
    pub next_token: Option<String>,
}

/// A page token is the key the NEXT page resumes after, hex-encoded.
///
/// Hex makes the token opaque in the sense the API promises — it is not a resource name a
/// client should read or construct, and it carries no meaning a caller may depend on —
/// but it is NOT authenticated: hex is reversible, and a hand-made token that decodes to
/// UTF-8 is accepted and simply resumes after that key. That is the same answer a token
/// whose record has since been deleted gets, which is what lets a listing resume
/// correctly across inserts and deletes rather than failing on them. Nothing a token can
/// say reaches past this listing's own key order, so an unauthenticated token grants no
/// access: a forged one can only start the same listing at a different place.
fn encode_page_token(key: &str) -> String {
    hex::encode(key)
}

/// The key a token resumes after, or the reason the token cannot be one this service
/// issued (its bytes are not hex, or not UTF-8).
fn decode_page_token(token: &str) -> Result<String, String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| {
            "nextToken is not a pagination token this service issued".to_string()
        })
}

/// The page of `entries` that `next_token` and `max_results` select.
///
/// `max_results` is a page BOUND, not a count: a page holds at most that many records,
/// and `next_token` is returned only while more remain.
pub fn page<'a, V>(
    entries: &'a BTreeMap<String, V>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, V>, String> {
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let bound = usize::try_from(bound).unwrap_or(1);
    let after = match next_token {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };

    let mut remaining = entries
        .iter()
        .filter(|(key, _)| after.as_deref().is_none_or(|from| key.as_str() > from));
    let selected: Vec<(&String, &V)> = remaining.by_ref().take(bound).collect();
    // A token only when a record is left over: the last page of a listing carries none.
    let next_token = remaining
        .next()
        .and_then(|_| selected.last().map(|(key, _)| encode_page_token(key)));

    Ok(Page {
        items: selected.into_iter().map(|(_, value)| value).collect(),
        next_token,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRedshiftserverlessRedshiftserverlessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRedshiftserverlessRedshiftserverlessState>)
}
