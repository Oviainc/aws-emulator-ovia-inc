// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-detective-amazondetective service.
//!
//! The service's primary resource is the **behavior graph**. A graph is stored
//! exactly once, keyed by its own ARN — the identifier every operation names it by
//! — and it owns the member accounts invited into it, so `CreateMembers`,
//! `ListMembers` and `GetMembers` all read and write the one record. Deleting the
//! graph therefore drops its members and its tags with it, which is what
//! `DeleteGraph` documents ("removes the behavior graph from each member account's
//! list of behavior graphs").
//!
//! `tags` lives on the graph record even though `TagResource`, `UntagResource` and
//! `ListTagsForResource` are still scaffolded: those three key off the same graph
//! ARN, so the map they will read is the one `CreateGraph` writes here.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{Graph as GraphDetail, MemberDetail, UnprocessedAccount};

/// Characters a behavior-graph id is built from.
///
/// Taken from the modelled `GraphArn` pattern, whose last segment is
/// `[abcdef\d]{32}`:
/// `smithy_query(query="shape",shape="GraphArn")`.
const GRAPH_ID_ALPHABET: &[u8] = b"0123456789abcdef";

/// Length of the id segment of a behavior-graph ARN, from the same pattern.
pub const GRAPH_ID_LEN: usize = 32;

/// The resource portion a behavior-graph ARN carries before its id, from the same
/// modelled pattern (`...:detective:<region>:<account>:graph:<id>`).
pub const GRAPH_ARN_RESOURCE_PREFIX: &str = "graph:";

/// The ARN service namespace for this service (`arnNamespace` / sigv4 name
/// `detective`).
pub const ARN_SERVICE: &str = "detective";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Length of an Amazon Web Services account identifier, from the modelled
/// `AccountId` (length 12..12, pattern `^[0-9]+$`).
pub const ACCOUNT_ID_LEN: usize = 12;

/// The most graphs this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. `ListGraphs` documents a far tighter
/// account-level rule ("Because an account can currently only be the administrator
/// of one behavior graph within a Region, the results always contain a single
/// behavior graph"), which this slice deliberately does not enforce: every request
/// this emulator serves is signed for the one canonical emulator account, so a
/// single-graph rule would leave `ListGraphs`' modelled pagination unreachable.
/// This cap is what keeps the store finite in its place, and a create that would
/// exceed it is refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_GRAPHS: usize = 500;

/// The most member accounts one behavior graph may hold.
///
/// From `ServiceQuotaExceededException`'s own documentation: "This request cannot
/// be completed if it would cause the number of member accounts in the behavior
/// graph to exceed the maximum allowed. A behavior graph cannot have more than
/// 1,200 member accounts."
pub const MAX_MEMBERS_PER_GRAPH: usize = 1_200;

/// The largest page either list operation will return.
///
/// From the modelled `MemberResultsLimit` range 1..200, which both `ListGraphs` and
/// `ListMembers` bind their `MaxResults` to; it is also the page size used when a
/// request names none.
pub const MAX_PAGE_SIZE: usize = 200;

/// `MemberStatus$VERIFICATION_IN_PROGRESS`: Detective is checking that the account
/// id and email address given for the member match. The value is one of
/// `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_VERIFICATION_IN_PROGRESS: &str = "VERIFICATION_IN_PROGRESS";

/// `MemberStatus$INVITED`: verification passed and the invitation has been sent,
/// but the member has not responded yet. One of `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_INVITED: &str = "INVITED";

/// `InvitationType$INVITATION`: the membership came from `CreateMembers` inviting
/// the account, not from an organization. One of `INVITATION_TYPE_VALUES`.
pub const INVITATION_TYPE_INVITATION: &str = "INVITATION";

/// How long after `CreateMembers` the verification it starts is due.
///
/// `MemberStatus$VERIFICATION_IN_PROGRESS` is a transient status Detective moves
/// out of by itself — "Detective is verifying that the account identifier and email
/// address provided for the member account match. If they do match, then Detective
/// sends the invitation" — so the move to `INVITED` is owed by the clock, not by any
/// later call. AWS publishes no duration for it; this is the span this emulator
/// chooses, short enough not to slow a caller down and long enough that a caller
/// reading straight back still observes the transient status.
pub const MEMBER_VERIFICATION_MILLIS: i64 = 500;

/// One stored member account of a behavior graph: everything `MemberDetail`
/// reports for it, plus the instant the verification `CreateMembers` started is
/// due.
#[derive(Debug, Clone)]
pub struct Member {
    pub account_id: String,
    pub email_address: String,
    pub graph_arn: String,
    /// The account that administers the graph this membership belongs to.
    pub administrator_id: String,
    /// One of `MEMBER_STATUS_VALUES`.
    pub status: String,
    /// One of `INVITATION_TYPE_VALUES`.
    pub invitation_type: String,
    pub invited_time_millis: i64,
    pub updated_time_millis: i64,
    /// One of `MEMBER_DISABLED_REASON_VALUES`, only while the membership is
    /// `ACCEPTED_BUT_DISABLED`.
    pub disabled_reason: Option<String>,
    /// The instant the `VERIFICATION_IN_PROGRESS` -> `INVITED` move is due, while
    /// that move is still outstanding.
    pub verification_due_millis: Option<i64>,
}

impl Member {
    /// Materialise the verification the clock already made due.
    ///
    /// This is the ONLY write a reporting operation makes: the move was owed at
    /// `verification_due_millis` whether or not anybody called, so `UpdatedTime`
    /// carries that due instant rather than the time of the read, and the record a
    /// later caller sees is identical.
    pub fn settle(&mut self, now_millis: i64) {
        if let Some(due) = self.verification_due_millis {
            if due <= now_millis {
                self.status = MEMBER_STATUS_INVITED.to_string();
                self.updated_time_millis = due;
                self.verification_due_millis = None;
            }
        }
    }

    /// This membership as the wire shape reports it.
    ///
    /// `MasterId` carries the same value as `AdministratorId`: the model deprecates
    /// it in favour of `AdministratorId` ("This property is deprecated. Use
    /// AdministratorId instead.") but still binds it, so a client reading the older
    /// member sees the administrator too. The volume and data-source-package members
    /// are left absent: nothing in this slice measures ingest, so reporting a number
    /// for them would be a fabrication.
    pub fn detail(&self) -> MemberDetail {
        MemberDetail {
            account_id: Some(self.account_id.clone()),
            email_address: Some(self.email_address.clone()),
            graph_arn: Some(self.graph_arn.clone()),
            master_id: Some(self.administrator_id.clone()),
            administrator_id: Some(self.administrator_id.clone()),
            status: Some(self.status.clone()),
            disabled_reason: self.disabled_reason.clone(),
            invited_time: Some(millis_to_date_time(self.invited_time_millis)),
            updated_time: Some(millis_to_date_time(self.updated_time_millis)),
            invitation_type: Some(self.invitation_type.clone()),
            ..Default::default()
        }
    }
}

/// One stored behavior graph: its ARN, when it was created, its tags and the
/// member accounts invited into it.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub arn: String,
    pub created_time_millis: i64,
    /// The account that created the graph, and so administers it.
    pub administrator_id: String,
    /// Keyed by tag key, so `ListTagsForResource` will walk a stable order.
    pub tags: BTreeMap<String, String>,
    /// Keyed by account id, so `ListMembers` walks a stable, total order and a page
    /// token can name the next account.
    pub members: BTreeMap<String, Member>,
}

impl Graph {
    /// This graph as `ListGraphs`' `Graph` shape reports it.
    pub fn detail(&self) -> GraphDetail {
        GraphDetail {
            arn: Some(self.arn.clone()),
            created_time: Some(millis_to_date_time(self.created_time_millis)),
        }
    }

    /// Materialise every member verification the clock already made due. See
    /// [`Member::settle`] for why a reporting operation is allowed to do this.
    pub fn settle_members(&mut self, now_millis: i64) {
        for member in self.members.values_mut() {
            member.settle(now_millis);
        }
    }
}

/// Per-service state for com-amazonaws-detective-amazondetective. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDetectiveAmazondetectiveState {
    /// Every behavior graph, keyed by its own ARN. A `BTreeMap` so `ListGraphs`
    /// walks a stable, total order and a page token can name the next ARN.
    pub graphs: BTreeMap<String, Graph>,
    /// Mixed into each generated graph id so two creates in the same nanosecond
    /// differ.
    id_counter: u64,
}

impl ComAmazonawsDetectiveAmazondetectiveState {
    /// The graph `arn` names, for a reporting operation.
    pub fn graph(&self, arn: &str) -> Option<&Graph> {
        self.graphs.get(arn)
    }

    /// The graph `arn` names, for an operation the spec says mutates it.
    pub fn graph_mut(&mut self, arn: &str) -> Option<&mut Graph> {
        self.graphs.get_mut(arn)
    }

    /// Store a newly created graph.
    pub fn insert_graph(&mut self, graph: Graph) {
        self.graphs.insert(graph.arn.clone(), graph);
    }

    /// Remove the graph `arn` names, with its members and its tags. `None` when
    /// nothing matched.
    pub fn remove_graph(&mut self, arn: &str) -> Option<Graph> {
        self.graphs.remove(arn)
    }

    /// A 32-character lowercase-hex graph id no stored graph in `region` is using.
    pub fn fresh_graph_arn(&mut self, region: &str) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let seed = now_nanos() ^ self.id_counter.rotate_left(17);
            let candidate = graph_arn(region, &graph_id_from_seed(seed));
            if !self.graphs.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded hex counters are also `[abcdef\d]{32}`, and there are always
        // more of them than there are stored graphs.
        (0u64..=self.graphs.len() as u64)
            .map(|n| graph_arn(region, &format!("{n:032x}")))
            .find(|arn| !self.graphs.contains_key(arn))
            .unwrap_or_else(|| graph_arn(region, &format!("{:032x}", self.graphs.len())))
    }
}

/// The ARN of the behavior graph `id` in `region`.
pub fn graph_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{GRAPH_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The account that administers the graphs this emulator serves: the one canonical
/// emulator account every ARN and every account field carries.
pub fn administrator_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only `[-\w]{2,20}` in that field, so anything else is
/// refused here and the default is used instead: a generated ARN can never carry
/// text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            (2..=20).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `arn` is shaped like a behavior-graph ARN this service mints.
///
/// The modelled `GraphArn` pattern is already enforced on every input member that
/// targets it, so this is only used to check a value the model cannot police — the
/// ARN carried inside an opaque pagination token.
pub fn is_graph_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(ARN_SERVICE), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let partition_ok = partition
        .strip_prefix("aws")
        .is_some_and(|rest| rest.len() <= 10 && rest.bytes().all(|b| b == b'-' || b == b'_' || b.is_ascii_alphanumeric()));
    let region_ok = (2..=20).contains(&region.len())
        && region
            .bytes()
            .all(|b| b == b'-' || b == b'_' || b.is_ascii_alphanumeric());
    let account_ok = account.len() == ACCOUNT_ID_LEN && account.bytes().all(|b| b.is_ascii_digit());
    let id_ok = resource
        .strip_prefix(GRAPH_ARN_RESOURCE_PREFIX)
        .is_some_and(|id| {
            id.len() == GRAPH_ID_LEN && id.bytes().all(|b| GRAPH_ID_ALPHABET.contains(&b))
        });
    partition_ok && region_ok && account_ok && id_ok
}

/// Whether `account_id` is shaped like the modelled `AccountId` (12 digits).
pub fn is_account_id(account_id: &str) -> bool {
    account_id.len() == ACCOUNT_ID_LEN && account_id.bytes().all(|b| b.is_ascii_digit())
}

/// The page size to serve, whatever the request asked for.
///
/// The size is the server's own decision — clamped into `1..=MAX_PAGE_SIZE` — so no
/// allocation a list handler makes is sized by caller input.
pub fn page_size(requested: Option<i32>) -> usize {
    requested.map_or(MAX_PAGE_SIZE, |value| {
        value.clamp(1, MAX_PAGE_SIZE as i32) as usize
    })
}

/// Encode an opaque pagination token naming the next item to return.
///
/// Hex rather than base64 so the token carries no character a protocol would have
/// to escape; it is opaque to a caller either way, and the longest key it wraps (a
/// graph ARN) renders well inside `PaginationToken`'s 1..1024 length bound.
pub fn encode_page_token(key: &str) -> String {
    ::hex::encode(key)
}

/// The key a pagination token names, or `None` when the token is not one this
/// service issued. The caller's own token text is never echoed back.
pub fn decode_page_token(token: &str) -> Option<String> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// An `UnprocessedAccount` naming `account_id` and why it was not processed.
pub fn unprocessed(account_id: &str, reason: String) -> UnprocessedAccount {
    UnprocessedAccount {
        account_id: Some(account_id.to_string()),
        reason: Some(reason),
    }
}

/// Now, in milliseconds since the Unix epoch — the resolution the ISO8601
/// `date-time` timestamps in this model carry ("For example,
/// `2021-08-18T16:35:56.284Z`").
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// An epoch-millisecond instant as the modelled `date-time` timestamp.
pub fn millis_to_date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `[abcdef\d]{32}` graph id from `seed`.
fn graph_id_from_seed(seed: u64) -> String {
    let mut state = seed;
    (0..GRAPH_ID_LEN)
        .map(|_| {
            let draw = splitmix64(&mut state);
            GRAPH_ID_ALPHABET[(draw % GRAPH_ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDetectiveAmazondetectiveState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDetectiveAmazondetectiveState>)
}
