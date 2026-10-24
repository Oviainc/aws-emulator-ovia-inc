// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wisdom-wisdomservice service.
//!
//! Wisdom's assistant lifecycle lives here: the assistants, the sessions nested
//! under each assistant, the knowledge bases, and the indexes that let a handler
//! resolve the `UuidOrArn` path labels the model declares (`^<uuid>$|^arn:…$`) back
//! to the one stored record. Handlers read and write ONLY through these structures,
//! so every response is computed from what a previous call persisted.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    AssistantAssociationData, AssistantAssociationOutputData, AssistantData, AssistantSummary,
    KnowledgeBaseAssociationData, KnowledgeBaseData, RecommendationData, RecommendationTrigger,
    RenderingConfiguration, ServerSideEncryptionConfiguration, SessionData, SourceConfiguration,
};

/// The ARN namespace Wisdom resources carry (`awsService.arnNamespace` in the
/// Smithy model), i.e. `arn:aws:wisdom:<region>:<account>:…`.
pub const ARN_NAMESPACE: &str = "wisdom";

/// Region used when a request carries no SigV4 credential scope to read one
/// from. It matches the region the emulator writes into the shared AWS config
/// it hands clients (`vendor/server/src/lib.rs`, `region = us-east-1`), so an
/// unsigned request still gets a well-formed ARN.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page `ListAssistants` serves. The model constrains `maxResults`
/// to `range 1..100`, so 100 is also the page size used when the caller names
/// none.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most assistants this store holds at once.
///
/// An assistant stays in the store until something deletes it, so an unbounded
/// store lets any caller that can reach the port grow this process's heap
/// without limit: the cap is the bound on that growth. It is a resource guard
/// this emulator owns, NOT an AWS-published quota — and a create past it is
/// refused with the error the model declares for exactly this on
/// `CreateAssistant`, a 402 `ServiceQuotaExceededException`.
pub const MAX_ASSISTANTS: usize = 1_000;

/// The most sessions one assistant holds.
///
/// Same guard, one level down: sessions are created under an assistant and are
/// only reclaimed when that assistant is deleted. `CreateSession` models no
/// quota error, so a create past this ceiling is refused with the modelled
/// `ConflictException` (409) — the conflict is the state of the parent
/// assistant, which holds as many sessions as it can.
pub const MAX_SESSIONS_PER_ASSISTANT: usize = 100;

/// The most sessions this store holds across every assistant, so the
/// assistants x sessions-per-assistant PRODUCT is finite and not just each
/// factor. Refused the same way as [`MAX_SESSIONS_PER_ASSISTANT`].
pub const MAX_STORED_SESSIONS: usize = 10_000;

/// The most assistant associations one assistant holds.
///
/// The same guard as [`MAX_SESSIONS_PER_ASSISTANT`], one level down from the
/// assistant: an association is reclaimed by `DeleteAssistantAssociation` or by
/// the delete of its assistant, so without a ceiling one assistant could be
/// associated without limit. `CreateAssistantAssociation` declares a quota error
/// for exactly this, so a create past it is refused with the modelled
/// `ServiceQuotaExceededException` (402). It is a resource guard this emulator
/// owns, NOT an AWS-published quota.
pub const MAX_ASSOCIATIONS_PER_ASSISTANT: usize = 100;

/// The only `AssociationType` this model declares
/// (`smithy_query(query="shape",shape="AssociationType")`), and so the only type
/// an association this service holds carries.
pub const ASSOCIATION_TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// The most `clientToken` replay entries the store remembers, oldest evicted
/// first.
///
/// A token's entry is otherwise reclaimed only when the resource it created is
/// deleted, so a caller that never deletes — or that retries with a fresh token
/// every time — would grow this index without limit. Past the ceiling the
/// oldest remembered token is forgotten, which makes a very old retry mint a
/// new resource rather than letting the index grow forever.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;

/// The most knowledge bases this store holds at once.
///
/// Same contract as [`MAX_ASSISTANTS`], one resource family over: a knowledge base
/// stays in the store until something deletes it, so the ceiling is the bound on
/// that growth. It is a resource guard this emulator owns, NOT an AWS-published
/// quota, and a create past it is refused with the error the model declares for
/// exactly this on `CreateKnowledgeBase`, a 402 `ServiceQuotaExceededException`.
pub const MAX_KNOWLEDGE_BASES: usize = 100;

/// The terminal `KnowledgeBaseStatus` of a usable knowledge base, and the one a
/// create answers with
/// (`smithy_query(query="shape",shape="KnowledgeBaseStatus")` also declares
/// `CREATE_IN_PROGRESS`, `CREATE_FAILED`, `DELETE_IN_PROGRESS`, `DELETE_FAILED` and
/// `DELETED`, none of which this emulator reaches: a delete removes the record
/// outright).
///
/// A recorded `CreateKnowledgeBase` answer carries `status` ACTIVE, so the create
/// owes no later transition — a resource the service returns ready stays ready.
pub const KNOWLEDGE_BASE_STATUS_ACTIVE: &str = "ACTIVE";

/// The sentence this endpoint answers a request addressed to a knowledge base this
/// account does not hold with.
///
/// It is a FIXED service text that quotes nothing from the request: the recorded wire
/// responses of this endpoint name no id in the message, and the Amazon Q in Connect
/// model of this same endpoint words it from its own copy of this constant
/// (`services/service-com-amazonaws-qconnect-wisdomservice/src/state.rs`'s
/// `MESSAGE_KNOWLEDGE_BASE_NOT_FOUND`) — the two are one endpoint, so a client must
/// not get a different sentence depending on which model it addressed. The requested
/// label still travels, in `ResourceNotFoundException`'s own `resourceName` member,
/// which the model provides for naming it.
pub const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The most tags one resource carries.
///
/// AWS's tagging restrictions put the maximum number of tags per resource at
/// 50, and this model declares no constraint on the `Tags` map, so without the
/// limit one create could store an arbitrarily large tag map. A request past it
/// is refused with the modelled `ValidationException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The sentence a tagging operation refuses an ARN this account holds no resource
/// under with.
///
/// It is the model's own documentation of `ResourceNotFoundException`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: "The specified
/// resource does not exist."), and it carries NOTHING from the request, so a caller
/// cannot probe the store through the wording. The ARN the caller asked about travels
/// in the error's own `resourceName` member, which the model provides for naming it.
pub const MESSAGE_RESOURCE_NOT_FOUND: &str = "The specified resource does not exist.";

/// The only `KnowledgeBaseType` a content upload may be started against.
///
/// `StartContentUpload`'s own documentation states the rule — "You can only upload
/// content to a knowledge base of type CUSTOM" — so a start against a knowledge base
/// of any other modelled type is refused instead of being handed a URL that could not
/// be used.
pub const KNOWLEDGE_BASE_TYPE_CUSTOM: &str = "CUSTOM";

/// How long an upload URL stays valid when the caller names no
/// `presignedUrlTimeToLive`.
///
/// CHOSEN, not documented: the model bounds that member at `range 1..60` minutes
/// (`smithy_query(query="operation",operation="StartContentUpload")`) and states no
/// default, so this emulator picks one inside those bounds. A caller that cares names
/// its own value, which is used as given.
pub const DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES: u64 = 30;

/// The most content uploads this store holds at once.
///
/// An upload is reclaimed when its URL expires or when its knowledge base is deleted,
/// so the store drains by itself; the ceiling bounds how much a caller that keeps
/// starting uploads can hold at any one instant. It is a resource guard this emulator
/// owns, NOT an AWS-published quota, and a start past it is refused with the modelled
/// `ValidationException` (the operation models no quota error).
pub const MAX_CONTENT_UPLOADS: usize = 1_000;

/// The most bytes one upload's `PUT` may carry.
///
/// The bytes live in this process's heap until the upload expires, so the body is
/// bounded the way the number of uploads is. It is a resource guard this emulator owns,
/// NOT an AWS-published quota, and a larger body is refused by the upload plane rather
/// than stored.
pub const MAX_CONTENT_UPLOAD_BYTES: usize = 8 * 1024 * 1024;

/// The header an upload's `PUT` carries the `contentType` the upload was started for
/// under: an upload URL is minted for ONE media type, exactly as an S3 presigned `PUT`
/// signed over a content type is.
pub const CONTENT_TYPE_HEADER: &str = "content-type";

/// The header naming the server-side encryption an encrypted knowledge base's upload
/// must be written with, and its value: a knowledge base that carries a
/// `serverSideEncryptionConfiguration` is written under that KMS key, so the upload
/// must say so.
pub const SERVER_SIDE_ENCRYPTION_HEADER: &str = "x-amz-server-side-encryption";
pub const SERVER_SIDE_ENCRYPTION_AWS_KMS: &str = "aws:kms";

/// The header naming the customer-managed KMS key an encrypted knowledge base's upload
/// is written under, carried only when the knowledge base names one.
pub const SERVER_SIDE_ENCRYPTION_KEY_ID_HEADER: &str =
    "x-amz-server-side-encryption-aws-kms-key-id";

/// The endpoint prefix this service publishes
/// (`smithy_query(query="service_info")`: endpoint templates
/// `https://wisdom.{Region}.{dnsSuffix}`), used to word an upload URL for a request
/// that addressed no endpoint host of its own.
pub const ENDPOINT_PREFIX: &str = "wisdom";

/// How many candidate ids a mint tries before giving up, so a collision does not make
/// the mint loop forever.
const MINT_ATTEMPTS: usize = 8;

/// The most recommendations one session's feed holds at once, and the most
/// triggers kept beside them.
///
/// A recommendation stays available until something acknowledges it
/// (`NotifyRecommendationsReceived`, still a scaffold stub with its own issue),
/// so an unbounded feed would let a session nobody acknowledges grow this
/// process's heap without limit: the cap is the bound on that growth. It is a
/// resource guard this emulator owns, NOT an AWS-published quota, and it equals
/// the largest page `GetRecommendations` can serve ([`MAX_PAGE_SIZE`], the
/// model's `maxResults` ceiling), so a full feed can still fill any page the
/// model allows. Publishing past it forgets the OLDEST entry, the way a live
/// feed drops what nobody collected rather than refusing new material.
pub const MAX_PENDING_RECOMMENDATIONS_PER_SESSION: usize = MAX_PAGE_SIZE as usize;

/// The recommendations one session has available, oldest first, with the
/// triggers that produced them.
///
/// `GetRecommendations` REPORTS this feed — the operation is `@readonly` — so a
/// read never consumes an entry: two reads of an unchanged feed answer alike,
/// and only an acknowledgement removes one. Nothing in THIS model's implemented
/// surface publishes a recommendation (real Wisdom makes them available out of
/// band, from the contact's detected intent), so a session's feed is empty
/// until something calls [`RecommendationFeed::publish`] — which is exactly the
/// "no recommendations available" case the operation documents: the call
/// returns successfully with an empty list.
#[derive(Debug, Clone, Default)]
pub struct RecommendationFeed {
    /// The available recommendations, oldest first: the order
    /// `GetRecommendations` serves a page in, and the order the cap evicts in.
    pub recommendations: VecDeque<RecommendationData>,
    /// The triggers that produced them, each naming the `recommendationId`s it
    /// produced (`RecommendationTrigger.recommendationIds`). A trigger is kept
    /// only while the feed still holds a recommendation it names, so a trigger
    /// that names none is not kept and not reported.
    pub triggers: Vec<RecommendationTrigger>,
}

impl RecommendationFeed {
    /// Make one recommendation available on this session, with the trigger that
    /// produced it (if any).
    ///
    /// Bounded by [`MAX_PENDING_RECOMMENDATIONS_PER_SESSION`]: the oldest
    /// recommendation is forgotten first, and a trigger no held recommendation
    /// names is forgotten with it.
    pub fn publish(
        &mut self,
        recommendation: RecommendationData,
        trigger: Option<RecommendationTrigger>,
    ) {
        self.recommendations.push_back(recommendation);
        if let Some(trigger) = trigger {
            self.triggers.push(trigger);
        }
        while self.recommendations.len() > MAX_PENDING_RECOMMENDATIONS_PER_SESSION {
            self.recommendations.pop_front();
        }
        let held: BTreeSet<String> = self
            .recommendations
            .iter()
            .map(|recommendation| recommendation.recommendation_id.clone())
            .collect();
        self.triggers.retain(|trigger| {
            trigger
                .recommendation_ids
                .iter()
                .any(|id| held.contains(id))
        });
        while self.triggers.len() > MAX_PENDING_RECOMMENDATIONS_PER_SESSION {
            self.triggers.remove(0);
        }
    }

    /// The page of at most `max_results` available recommendations, oldest
    /// first, together with the triggers that produced the recommendations IN
    /// that page.
    pub fn page(&self, max_results: usize) -> (Vec<RecommendationData>, Vec<RecommendationTrigger>) {
        let recommendations: Vec<RecommendationData> = self
            .recommendations
            .iter()
            .take(max_results)
            .cloned()
            .collect();
        let paged: BTreeSet<&str> = recommendations
            .iter()
            .map(|recommendation| recommendation.recommendation_id.as_str())
            .collect();
        let triggers: Vec<RecommendationTrigger> = self
            .triggers
            .iter()
            .filter(|trigger| {
                trigger
                    .recommendation_ids
                    .iter()
                    .any(|id| paged.contains(id.as_str()))
            })
            .cloned()
            .collect();
        (recommendations, triggers)
    }
}

/// A persisted Wisdom assistant, plus the sessions created under it.
#[derive(Debug, Clone)]
pub struct Assistant {
    pub assistant_id: String,
    pub assistant_arn: String,
    pub name: String,
    /// `AssistantType` on the wire (the model's only value is `AGENT`).
    pub assistant_type: String,
    /// `AssistantStatus` on the wire.
    pub status: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    pub sessions: HashMap<String, Session>,
    /// Session ids in creation order, so a session listing is stable.
    pub session_order: Vec<String>,
    /// When this record was created. Kept as the record's own creation instant —
    /// `AssistantData`/`AssistantSummary` model no timestamp member, so nothing
    /// renders it; it is what a later operation that needs an age (an expiry, a
    /// creation-time sort) reads, and it is never derived from the request.
    pub created_at: SystemTime,
}

/// A persisted Wisdom session, owned by exactly one assistant.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub session_arn: String,
    pub assistant_id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// When this session was created. `SessionData` models no timestamp member,
    /// so nothing renders it; see [`Assistant::created_at`].
    pub created_at: SystemTime,
    /// The recommendations available on this session, which
    /// `GetRecommendations` reports. It lives ON the session, so the delete of
    /// the owning assistant reclaims it with the session rather than leaving an
    /// orphan feed behind in a map beside the sessions.
    pub recommendations: RecommendationFeed,
}

/// A persisted association between an assistant and a knowledge base.
///
/// An association is reachable only through its assistant — every modelled route
/// is `/assistants/{assistantId}/associations/…` — so [`Self::assistant_id`] is
/// what makes a lookup under another assistant miss.
#[derive(Debug, Clone)]
pub struct AssistantAssociation {
    pub assistant_association_id: String,
    pub assistant_association_arn: String,
    pub assistant_id: String,
    pub assistant_arn: String,
    /// `AssociationType` on the wire; this model declares the single value
    /// [`ASSOCIATION_TYPE_KNOWLEDGE_BASE`].
    pub association_type: String,
    /// The knowledge base this association was made against, kept by id together
    /// with the ARN it carried then, so a read reports the knowledge base the
    /// association was made against even once that knowledge base is gone.
    pub knowledge_base_id: String,
    pub knowledge_base_arn: String,
    pub tags: BTreeMap<String, String>,
    /// The fingerprint of the create that minted this record — see
    /// [`association_fingerprint`]. What a replayed `clientToken` is compared
    /// against, so a token reused with different parameters is told it conflicts
    /// instead of being handed a resource it did not ask for.
    pub fingerprint: String,
    /// When this association was created. `AssistantAssociationData` models no
    /// timestamp member, so nothing renders it; see [`Assistant::created_at`].
    pub created_at: SystemTime,
}

impl AssistantAssociation {
    /// This association as the `AssistantAssociationData` shape
    /// `CreateAssistantAssociation` and `GetAssistantAssociation` return.
    pub fn to_data(&self) -> AssistantAssociationData {
        AssistantAssociationData {
            assistant_association_id: self.assistant_association_id.clone(),
            assistant_association_arn: self.assistant_association_arn.clone(),
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            association_type: self.association_type.clone(),
            association_data: AssistantAssociationOutputData::knowledgeBaseAssociation(
                KnowledgeBaseAssociationData {
                    knowledge_base_id: Some(self.knowledge_base_id.clone()),
                    knowledge_base_arn: Some(self.knowledge_base_arn.clone()),
                },
            ),
            tags: wire_tags(&self.tags),
        }
    }
}

/// The fingerprint of a `CreateAssistantAssociation` request: what its
/// `clientToken` identifies.
///
/// An idempotency token identifies ONE request, so a replay carrying the
/// identical request is a retry and answers with the resource the first call
/// created, while the same token carrying a DIFFERENT request is an idempotency
/// conflict and is refused. The fingerprint therefore covers every member the
/// created record is made of — the association type, the associated knowledge
/// base and the tags — and nothing else; the tag pairs are sorted, so the same
/// request written in another order is one request.
pub fn association_fingerprint(
    association_type: &str,
    knowledge_base_id: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let pairs: Vec<String> = tags.iter().map(|(key, tag)| format!("{key}={tag}")).collect();
    format!(
        "{association_type}\u{1f}{knowledge_base_id}\u{1f}{}",
        pairs.join("\u{1e}")
    )
}

/// The `tags` member of a resource this service reports: ALWAYS present, and an
/// empty object when the resource carries no tag.
///
/// This is what the endpoint answers on the wire — a recorded create of an
/// untagged resource carries the member all the same — so a client reading
/// `response["tags"]` finds a map for every resource instead of telling an
/// untagged resource's absent member from a tagged one's map. The Amazon Q in
/// Connect model of this same service states the rule in the same way (that
/// crate's `state::tags_out`), so the two mounted models of one endpoint cannot
/// answer this member differently.
///
/// It is the ONE owner of that rule in this crate: every operation that reports a
/// `tags` member — a resource read, and `ListTagsForResource`, whose whole output
/// is that member — renders it through here rather than restating the rule.
pub fn wire_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

impl Assistant {
    /// This assistant as the `AssistantData` shape `CreateAssistant` and
    /// `GetAssistant` return.
    pub fn to_data(&self) -> AssistantData {
        AssistantData {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            name: self.name.clone(),
            r#type: self.assistant_type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: wire_tags(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // Only an assistant wired to a Connect topic carries one; this
            // emulator never creates that wiring, so the member stays absent.
            integration_configuration: None,
        }
    }

    /// This assistant as the `AssistantSummary` shape `ListAssistants` returns.
    pub fn to_summary(&self) -> AssistantSummary {
        AssistantSummary {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            name: self.name.clone(),
            r#type: self.assistant_type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: wire_tags(&self.tags),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            integration_configuration: None,
        }
    }
}

impl Session {
    /// This session as the `SessionData` shape `CreateSession` and `GetSession`
    /// return.
    pub fn to_data(&self) -> SessionData {
        SessionData {
            session_arn: self.session_arn.clone(),
            session_id: self.session_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            tags: wire_tags(&self.tags),
            integration_configuration: None,
        }
    }
}

/// A persisted Wisdom knowledge base.
///
/// The knowledge base is the root of the content/quick-response side of this service,
/// the way [`Assistant`] is the root of the session side; nothing is nested under it
/// yet, because every content operation is still a scaffold. It is also the record a
/// `CreateAssistantAssociation` resolves its `knowledgeBaseId` against, so there is
/// ONE knowledge-base store in this crate rather than a second one beside it.
#[derive(Debug, Clone)]
pub struct KnowledgeBase {
    pub knowledge_base_id: String,
    pub knowledge_base_arn: String,
    pub name: String,
    /// `KnowledgeBaseType` on the wire (`CUSTOM`, `EXTERNAL` or `QUICK_RESPONSES`).
    pub knowledge_base_type: String,
    /// `KnowledgeBaseStatus` on the wire.
    pub status: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub source_configuration: Option<SourceConfiguration>,
    pub rendering_configuration: Option<RenderingConfiguration>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    /// When this record was created, as [`Assistant`] keeps it too.
    /// `KnowledgeBaseData` models no creation-time member, so nothing renders it; it
    /// is the record's own age and is never derived from the request.
    pub created_at: SystemTime,
}

impl KnowledgeBase {
    /// This knowledge base as the `KnowledgeBaseData` shape `CreateKnowledgeBase` and
    /// `GetKnowledgeBase` return.
    pub fn to_data(&self) -> KnowledgeBaseData {
        KnowledgeBaseData {
            knowledge_base_id: self.knowledge_base_id.clone(),
            knowledge_base_arn: self.knowledge_base_arn.clone(),
            name: self.name.clone(),
            knowledge_base_type: self.knowledge_base_type.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            tags: wire_tags(&self.tags),
            source_configuration: self.source_configuration.clone(),
            rendering_configuration: self.rendering_configuration.clone(),
            server_side_encryption_configuration: self
                .server_side_encryption_configuration
                .clone(),
            // The model's own wording for the member: "If no content exists in a
            // knowledge base, this value is unset". No content operation is
            // implemented in this view of the endpoint, so no knowledge base it holds
            // has content whose modification time there would be to report.
            last_content_modification_time: None,
        }
    }
}

/// One content upload `StartContentUpload` opened: the slot the URL it handed out
/// writes into.
///
/// The record is what makes that URL a locator for something this emulator holds: the
/// upload plane (`crate::service`) serves a `PUT` to it out of this record, and
/// `CreateContent`/`UpdateContent` name the same `uploadId` to finalize the asset. The
/// published headers are STORED rather than recomputed, so the headers the start call
/// told the caller to include are exactly the ones the `PUT` is held to.
#[derive(Debug, Clone)]
pub struct ContentUpload {
    pub upload_id: String,
    /// The `knowledgeBaseId` the upload was started under — the resolved id, never the
    /// label form the request used, so an upload resolves the same way by id or by ARN.
    pub knowledge_base_id: String,
    /// `ContentType` on the wire: the one media type this upload accepts.
    pub content_type: String,
    /// The URL handed to the caller, kept so the record and the locator cannot drift.
    pub url: String,
    /// When the URL stops being usable. A `PUT` at or after it is refused and the
    /// record is reclaimed.
    pub url_expiry: SystemTime,
    /// The headers the start call published, which the `PUT` must carry.
    pub headers_to_include: BTreeMap<String, String>,
    pub started_at: SystemTime,
    /// The bytes a `PUT` to the URL stored, `None` until one does.
    pub body: Option<Vec<u8>>,
    /// When those bytes arrived.
    pub received_at: Option<SystemTime>,
}

impl ContentUpload {
    /// Whether this upload's URL is no longer usable at `now`.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        now >= self.url_expiry
    }

    /// Whether a request carries every header this upload was started with, each with
    /// the published value. `sent` reads one header off the request by name (HTTP header
    /// names are case-insensitive, and the published names are already lowercase).
    ///
    /// This is what `headersToInclude` MEANS: the upload is written only by a request
    /// that includes them, so a caller that echoes the start call's map is accepted and
    /// one that drops a header is refused.
    pub fn headers_satisfied<F>(&self, sent: F) -> bool
    where
        F: Fn(&str) -> Option<String>,
    {
        self.headers_to_include
            .iter()
            .all(|(name, value)| sent(name).as_deref() == Some(value.as_str()))
    }
}

/// What a request to a published upload URL names: the knowledge-base label and the
/// upload id, as [`content_upload_path`] words them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUploadTarget {
    pub knowledge_base_label: String,
    pub upload_id: String,
}

/// The path the upload plane serves one upload under.
///
/// It sits under the knowledge base the upload belongs to — the same `/knowledgeBases`
/// space the modelled routes use — and names no modelled operation (the model's own
/// upload route is `POST /knowledgeBases/{knowledgeBaseId}/upload`), which is what lets
/// the service serve it from its routing-error seam.
pub fn content_upload_path(knowledge_base_id: &str, upload_id: &str) -> String {
    format!("/knowledgeBases/{knowledge_base_id}/uploads/{upload_id}")
}

/// The upload a request path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_content_upload_path(path: &str) -> Option<ContentUploadTarget> {
    let rest = path.strip_prefix("/knowledgeBases/")?;
    let mut segments = rest.split('/');
    let (Some(knowledge_base_label), Some("uploads"), Some(upload_id), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if knowledge_base_label.is_empty() || upload_id.is_empty() {
        return None;
    }
    Some(ContentUploadTarget {
        knowledge_base_label: knowledge_base_label.to_string(),
        upload_id: upload_id.to_string(),
    })
}

/// The URL an upload is written to: the endpoint the caller reached, plus the upload's
/// own path.
///
/// NO SigV4 signature is attached — this emulator does not sign URLs, and a fabricated
/// signature would claim a computation nothing performed. The upload id is the receipt
/// that names the slot, and it is minted, unguessable and expiring.
pub fn content_upload_url(host: &str, knowledge_base_id: &str, upload_id: &str) -> String {
    format!(
        "https://{host}{}",
        content_upload_path(knowledge_base_id, upload_id)
    )
}

/// The endpoint host a request addressed, so a URL this service hands out is reachable
/// at the endpoint the caller is already talking to; [`ENDPOINT_PREFIX`]'s published
/// host for a request that named none.
///
/// The header is only taken when it LOOKS like a host — letters, digits, `-`, `.` and a
/// `:port` — so a caller cannot get an arbitrary string of its own into the URL this
/// service reports.
pub fn request_endpoint_host(headers: &::http::HeaderMap, region: &str) -> String {
    headers
        .get("host")
        .and_then(|host| host.to_str().ok())
        .filter(|host| is_host_shaped(host))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ENDPOINT_PREFIX}.{region}.amazonaws.com"))
}

/// Whether `host` is a non-empty `host[:port]` of the characters a hostname carries.
fn is_host_shaped(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':'))
}

/// What makes a `clientToken` replay the SAME create: the operation, the scope
/// the created resource lives in, and the token itself.
///
/// The scope matters because a token is only unique within the collection it
/// created into: `CreateSession` creates under one assistant, so the same token
/// used under a different assistant is a different create, not a retry of the
/// first. It is the empty string for an account-level create (`CreateAssistant`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub operation: String,
    pub scope: String,
    pub client_token: String,
}

/// One entry of the assistant listing: the assistant, plus the cursor the
/// listing issued it at.
///
/// A `nextToken` names a CURSOR, never a position or an id, so an assistant
/// deleted between two pages cannot invalidate a token already handed out: the
/// continuation resumes at the first surviving entry at or after that cursor.
#[derive(Debug, Clone)]
pub struct ListingEntry {
    pub cursor: u64,
    pub assistant_id: String,
}

/// How a [`ComAmazonawsWisdomWisdomserviceState::insert_session`] ended: stored,
/// refused because the parent assistant is gone, or refused because a session
/// ceiling is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionInsert {
    /// The session is in the store under its parent.
    Stored,
    /// The parent assistant does not exist (any more).
    UnknownAssistant,
    /// [`MAX_SESSIONS_PER_ASSISTANT`] or [`MAX_STORED_SESSIONS`] is reached.
    AtCapacity,
}

/// Per-service state for com-amazonaws-wisdom-wisdomservice. A handler reaches it through [`service_state`].
///
/// Every caller-driven store here is BOUNDED, so repeated requests cannot grow
/// this process's heap without limit: `assistants` (and the indexes and the
/// listing that mirror it, one entry per assistant) holds at most
/// [`MAX_ASSISTANTS`] records; the sessions nested under them hold at most
/// [`MAX_SESSIONS_PER_ASSISTANT`] each and [`MAX_STORED_SESSIONS`] in total, so
/// the product of the two ceilings cannot multiply out; each record's tag map
/// holds at most [`MAX_TAGS_PER_RESOURCE`] pairs; and `idempotency` holds at
/// most [`MAX_REMEMBERED_CLIENT_TOKENS`] entries, evicting the oldest first; and
/// `knowledge_bases` (with the two indexes that mirror it) holds at most
/// [`MAX_KNOWLEDGE_BASES`] records.
#[derive(Debug, Default)]
pub struct ComAmazonawsWisdomWisdomserviceState {
    /// Every assistant, keyed by its `assistantId` (a v4 uuid).
    pub assistants: HashMap<String, Assistant>,
    /// The assistants in creation order: the page order `ListAssistants` serves.
    pub assistant_order: Vec<ListingEntry>,
    /// The cursor the next created assistant takes. Monotonic, so a cursor is
    /// never reused and a page token stays meaningful after a delete.
    pub next_cursor: u64,
    /// `name` -> `assistantId`, so a duplicate name is a `ConflictException`.
    pub assistant_name_index: HashMap<String, String>,
    /// `assistantArn` -> `assistantId`, so an ARN-form label resolves to the
    /// same record an id-form label does.
    pub assistant_arn_index: HashMap<String, String>,
    /// `sessionArn` -> (`assistantId`, `sessionId`), the same for session labels.
    pub session_arn_index: HashMap<String, (String, String)>,
    /// Every knowledge base this account holds, keyed by its `knowledgeBaseId` (a
    /// v4 uuid) — the record `CreateKnowledgeBase` mints and the one a
    /// `CreateAssistantAssociation` resolves its `knowledgeBaseId` against, so the
    /// two cannot disagree about one knowledge base.
    pub knowledge_bases: HashMap<String, KnowledgeBase>,
    /// `name` -> `knowledgeBaseId`, so a duplicate name is a `ConflictException`.
    pub knowledge_base_name_index: HashMap<String, String>,
    /// `knowledgeBaseArn` -> `knowledgeBaseId`, so an ARN-form label resolves to the
    /// same record an id-form label does.
    pub knowledge_base_arn_index: HashMap<String, String>,
    /// Every assistant association, keyed by its `assistantAssociationId`. Each
    /// record names the assistant it belongs to, so a lookup is always scoped to
    /// the parent the route named.
    pub assistant_associations: HashMap<String, AssistantAssociation>,
    /// `assistantAssociationArn` -> (`assistantId`, `assistantAssociationId`), so
    /// an ARN-form label resolves to the same record an id-form label does.
    pub assistant_association_arn_index: HashMap<String, (String, String)>,
    /// How many sessions `assistants` holds in total, for the store-wide
    /// session ceiling ([`MAX_STORED_SESSIONS`]).
    pub stored_sessions: usize,
    /// What each `clientToken` already created, for create replay.
    pub idempotency: HashMap<IdempotencyKey, String>,
    /// The remembered tokens in insertion order, so the index evicts the oldest
    /// first once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held.
    pub idempotency_order: VecDeque<IdempotencyKey>,
    /// Every content upload `StartContentUpload` opened, keyed by its `uploadId`, at
    /// most [`MAX_CONTENT_UPLOADS`] of them and each holding at most
    /// [`MAX_CONTENT_UPLOAD_BYTES`] bytes. An entry is reclaimed once its URL expires
    /// or its knowledge base is deleted.
    pub content_uploads: HashMap<String, ContentUpload>,
}

impl ComAmazonawsWisdomWisdomserviceState {
    /// Resolve an assistant path label — a bare uuid or an assistant ARN — to a
    /// stored `assistantId`. `None` means no such assistant.
    pub fn resolve_assistant_id(&self, label: &str) -> Option<String> {
        if label.starts_with("arn:") {
            self.assistant_arn_index.get(label).cloned()
        } else {
            self.assistants
                .contains_key(label)
                .then(|| label.to_string())
        }
    }

    /// The assistant a path label names, in either label form.
    pub fn assistant(&self, label: &str) -> Option<&Assistant> {
        let id = self.resolve_assistant_id(label)?;
        self.assistants.get(&id)
    }

    /// The session a (assistant label, session label) pair names. A session
    /// whose owner is a different assistant does not resolve.
    pub fn session(&self, assistant_label: &str, session_label: &str) -> Option<&Session> {
        let assistant_id = self.resolve_assistant_id(assistant_label)?;
        let session_id = if session_label.starts_with("arn:") {
            let (owner, session_id) = self.session_arn_index.get(session_label)?;
            if *owner != assistant_id {
                return None;
            }
            session_id.clone()
        } else {
            session_label.to_string()
        };
        self.assistants.get(&assistant_id)?.sessions.get(&session_id)
    }

    /// The same session as [`Self::session`], for a caller that writes to it —
    /// the seam a producer of recommendations ([`RecommendationFeed::publish`])
    /// reaches a session's feed through.
    pub fn session_mut(
        &mut self,
        assistant_label: &str,
        session_label: &str,
    ) -> Option<&mut Session> {
        let assistant_id = self.resolve_assistant_id(assistant_label)?;
        let session_id = if session_label.starts_with("arn:") {
            let (owner, session_id) = self.session_arn_index.get(session_label)?;
            if *owner != assistant_id {
                return None;
            }
            session_id.clone()
        } else {
            session_label.to_string()
        };
        self.assistants
            .get_mut(&assistant_id)?
            .sessions
            .get_mut(&session_id)
    }

    /// Whether this store already holds [`MAX_ASSISTANTS`] assistants, i.e.
    /// whether a create must be refused rather than add another record.
    pub fn assistants_at_capacity(&self) -> bool {
        self.assistants.len() >= MAX_ASSISTANTS
    }

    /// Persist a new assistant and index it by name and ARN. `false` — nothing
    /// stored — when the store is already at [`MAX_ASSISTANTS`], so the bound
    /// holds however the caller reaches this.
    pub fn insert_assistant(&mut self, assistant: Assistant) -> bool {
        if self.assistants_at_capacity() {
            return false;
        }
        let cursor = self.next_cursor;
        self.next_cursor += 1;
        self.assistant_order.push(ListingEntry {
            cursor,
            assistant_id: assistant.assistant_id.clone(),
        });
        self.assistant_name_index
            .insert(assistant.name.clone(), assistant.assistant_id.clone());
        self.assistant_arn_index
            .insert(assistant.assistant_arn.clone(), assistant.assistant_id.clone());
        self.assistants
            .insert(assistant.assistant_id.clone(), assistant);
        true
    }

    /// Persist a new session under its (already resolved) parent assistant,
    /// within both session ceilings.
    pub fn insert_session(&mut self, session: Session) -> SessionInsert {
        // Read the store-wide occupancy before the parent is borrowed, so the
        // parent is still resolved FIRST: a create naming an assistant that
        // does not exist is not found, whatever the store's occupancy.
        let store_full = self.stored_sessions >= MAX_STORED_SESSIONS;
        let Some(assistant) = self.assistants.get_mut(&session.assistant_id) else {
            return SessionInsert::UnknownAssistant;
        };
        if store_full || assistant.sessions.len() >= MAX_SESSIONS_PER_ASSISTANT {
            return SessionInsert::AtCapacity;
        }
        assistant.session_order.push(session.session_id.clone());
        self.session_arn_index.insert(
            session.session_arn.clone(),
            (session.assistant_id.clone(), session.session_id.clone()),
        );
        assistant
            .sessions
            .insert(session.session_id.clone(), session);
        self.stored_sessions += 1;
        SessionInsert::Stored
    }

    /// Drop an assistant, its sessions, its associations and every index entry
    /// that named them — including the `clientToken` entries that created them,
    /// so a create/delete cycle reclaims everything it allocated instead of
    /// retaining a row per token for the life of the process.
    pub fn remove_assistant(&mut self, assistant_id: &str) -> Option<Assistant> {
        let assistant = self.assistants.remove(assistant_id)?;
        self.assistant_order
            .retain(|entry| entry.assistant_id != assistant_id);
        self.assistant_name_index.remove(&assistant.name);
        self.assistant_arn_index.remove(&assistant.assistant_arn);
        let mut removed: std::collections::HashSet<String> =
            std::collections::HashSet::from([assistant_id.to_string()]);
        for session in assistant.sessions.values() {
            self.session_arn_index.remove(&session.session_arn);
            removed.insert(session.session_id.clone());
        }
        // An association exists only inside its assistant, so it goes with it.
        let associations: Vec<String> = self
            .assistant_associations
            .values()
            .filter(|association| association.assistant_id == assistant_id)
            .map(|association| association.assistant_association_id.clone())
            .collect();
        for id in associations {
            if let Some(association) = self.assistant_associations.remove(&id) {
                self.assistant_association_arn_index
                    .remove(&association.assistant_association_arn);
            }
            removed.insert(id);
        }
        self.stored_sessions = self.stored_sessions.saturating_sub(assistant.sessions.len());
        self.idempotency
            .retain(|_, resource_id| !removed.contains(resource_id.as_str()));
        // The order queue mirrors the map, so it drops exactly the same entries.
        self.idempotency_order
            .retain(|key| self.idempotency.contains_key(key));
        Some(assistant)
    }

    /// The tags persisted on the resource a full resource ARN names, whatever
    /// resource family that ARN belongs to.
    ///
    /// Tagging is a cross-family surface: `ListTagsForResource`, `TagResource` and
    /// `UntagResource` take an `Arn` rather than one family's path label
    /// (`smithy_query(query="operation",operation="ListTagsForResource")`), so the
    /// lookup goes through the ARN index of EVERY family this store holds a tag map
    /// for — assistants, knowledge bases, sessions and assistant associations — and
    /// reads the tags off that one stored record. Nothing is copied into a second
    /// place: a tag a create persisted on its resource is the tag reported here, and a
    /// resource a delete removed is in no index any more.
    ///
    /// `None` means the ARN names no resource this account holds, which includes an
    /// ARN this emulator never minted (another partition, service or account): it is
    /// in no index, so it names nothing.
    pub fn tags_of_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(assistant_id) = self.assistant_arn_index.get(resource_arn) {
            return self
                .assistants
                .get(assistant_id)
                .map(|assistant| &assistant.tags);
        }
        if let Some(knowledge_base_id) = self.knowledge_base_arn_index.get(resource_arn) {
            return self
                .knowledge_bases
                .get(knowledge_base_id)
                .map(|knowledge_base| &knowledge_base.tags);
        }
        if let Some((assistant_id, session_id)) = self.session_arn_index.get(resource_arn) {
            return self
                .assistants
                .get(assistant_id)
                .and_then(|assistant| assistant.sessions.get(session_id))
                .map(|session| &session.tags);
        }
        if let Some((_, association_id)) = self.assistant_association_arn_index.get(resource_arn) {
            return self
                .assistant_associations
                .get(association_id)
                .map(|association| &association.tags);
        }
        None
    }

    /// Resolve a knowledge base path label — a bare uuid or a knowledge-base ARN — to
    /// a stored `knowledgeBaseId`. `None` means no such knowledge base, which includes
    /// an ARN this emulator never minted: it is in no index, so it names nothing.
    pub fn resolve_knowledge_base_id(&self, label: &str) -> Option<String> {
        if label.starts_with("arn:") {
            self.knowledge_base_arn_index.get(label).cloned()
        } else {
            self.knowledge_bases
                .contains_key(label)
                .then(|| label.to_string())
        }
    }

    /// The knowledge base a path label names, in either label form. A caller whose
    /// modelled member is a bare `Uuid` (`CreateAssistantAssociation`'s
    /// `knowledgeBaseId`) simply never reaches the ARN branch.
    pub fn knowledge_base(&self, label: &str) -> Option<&KnowledgeBase> {
        let id = self.resolve_knowledge_base_id(label)?;
        self.knowledge_bases.get(&id)
    }

    /// Whether this store already holds [`MAX_KNOWLEDGE_BASES`] knowledge bases, i.e.
    /// whether a create must be refused rather than add another record.
    pub fn knowledge_bases_at_capacity(&self) -> bool {
        self.knowledge_bases.len() >= MAX_KNOWLEDGE_BASES
    }

    /// Persist a new knowledge base and index it by name and ARN. `false` — nothing
    /// stored — when the store is already at [`MAX_KNOWLEDGE_BASES`], so the bound
    /// holds however the caller reaches this.
    pub fn insert_knowledge_base(&mut self, knowledge_base: KnowledgeBase) -> bool {
        if self.knowledge_bases_at_capacity() {
            return false;
        }
        self.knowledge_base_name_index.insert(
            knowledge_base.name.clone(),
            knowledge_base.knowledge_base_id.clone(),
        );
        self.knowledge_base_arn_index.insert(
            knowledge_base.knowledge_base_arn.clone(),
            knowledge_base.knowledge_base_id.clone(),
        );
        self.knowledge_bases
            .insert(knowledge_base.knowledge_base_id.clone(), knowledge_base);
        true
    }

    /// Drop a knowledge base and every index entry that named it — including the
    /// `clientToken` entry that created it, so a create/delete cycle reclaims
    /// everything it allocated and a replayed token cannot resurrect a deleted record.
    ///
    /// The name goes with it, so a later create may take that name again: a duplicate
    /// name conflicts with a LIVE knowledge base, not with one that is gone. `None`
    /// means the label named no knowledge base, which is the caller's not-found.
    ///
    /// An association made against this knowledge base is NOT dropped with it: the
    /// association keeps the id and the ARN it was made against ([`AssistantAssociation`]),
    /// so a read of it still reports the knowledge base it names.
    pub fn remove_knowledge_base(&mut self, label: &str) -> Option<KnowledgeBase> {
        let id = self.resolve_knowledge_base_id(label)?;
        let knowledge_base = self.knowledge_bases.remove(&id)?;
        self.knowledge_base_name_index.remove(&knowledge_base.name);
        self.knowledge_base_arn_index
            .remove(&knowledge_base.knowledge_base_arn);
        self.idempotency.retain(|_, resource_id| *resource_id != id);
        // The order queue mirrors the map, so it drops exactly the same entries.
        self.idempotency_order
            .retain(|key| self.idempotency.contains_key(key));
        // An upload URL is a locator INSIDE the knowledge base it was started under, so
        // a knowledge base that is gone takes its open uploads with it: following such a
        // URL afterwards names nothing, and the bytes it held are reclaimed.
        self.content_uploads
            .retain(|_, upload| upload.knowledge_base_id != id);
        Some(knowledge_base)
    }

    /// The upload an `uploadId` names, whatever its state.
    pub fn content_upload(&self, upload_id: &str) -> Option<&ContentUpload> {
        self.content_uploads.get(upload_id)
    }

    /// The upload an `uploadId` names, for the plane that writes its bytes.
    pub fn content_upload_mut(&mut self, upload_id: &str) -> Option<&mut ContentUpload> {
        self.content_uploads.get_mut(upload_id)
    }

    /// Forget every upload whose URL has expired by `now`.
    ///
    /// This materialises what the CLOCK already made due — an expired URL cannot be
    /// used by anyone, so the slot was owed back at the instant it expired, whether or
    /// not anybody called — and it is what keeps the store draining rather than growing
    /// to [`MAX_CONTENT_UPLOADS`] and staying there.
    pub fn reclaim_expired_content_uploads(&mut self, now: SystemTime) {
        self.content_uploads
            .retain(|_, upload| !upload.has_expired(now));
    }

    /// Whether this store already holds [`MAX_CONTENT_UPLOADS`] uploads, i.e. whether a
    /// start must be refused rather than open another one.
    pub fn content_uploads_at_capacity(&self) -> bool {
        self.content_uploads.len() >= MAX_CONTENT_UPLOADS
    }

    /// Persist an opened upload. `false` — nothing stored — when the store is already at
    /// [`MAX_CONTENT_UPLOADS`], so the bound holds however the caller reaches this.
    pub fn insert_content_upload(&mut self, upload: ContentUpload) -> bool {
        if self.content_uploads_at_capacity() {
            return false;
        }
        self.content_uploads
            .insert(upload.upload_id.clone(), upload);
        true
    }

    /// An `uploadId` no stored upload holds, or `None` when [`MINT_ATTEMPTS`] candidates
    /// were all taken.
    ///
    /// The model constrains `UploadId` by length alone (`len 1..1200`, no pattern), so a
    /// minted v4 uuid satisfies it, and it is unguessable — which is what makes the id
    /// itself the receipt the upload URL carries.
    pub fn mint_content_upload_id(&self) -> Option<String> {
        (0..MINT_ATTEMPTS)
            .map(|_| new_resource_id())
            .find(|candidate| !self.content_uploads.contains_key(candidate))
    }

    /// The association a (assistant label, association label) pair names, in
    /// either label form. An association whose owner is a different assistant
    /// does not resolve, the way a session of another assistant does not.
    pub fn assistant_association(
        &self,
        assistant_label: &str,
        association_label: &str,
    ) -> Option<&AssistantAssociation> {
        let assistant_id = self.resolve_assistant_id(assistant_label)?;
        let association_id = if association_label.starts_with("arn:") {
            let (owner, association_id) = self
                .assistant_association_arn_index
                .get(association_label)?;
            if *owner != assistant_id {
                return None;
            }
            association_id.clone()
        } else {
            association_label.to_string()
        };
        self.assistant_associations
            .get(&association_id)
            .filter(|association| association.assistant_id == assistant_id)
    }

    /// Every association of one (resolved) assistant, in
    /// `assistantAssociationId` order, so what a read reports does not depend on
    /// map iteration order.
    pub fn assistant_associations_of(&self, assistant_id: &str) -> Vec<&AssistantAssociation> {
        let mut associations: Vec<&AssistantAssociation> = self
            .assistant_associations
            .values()
            .filter(|association| association.assistant_id == assistant_id)
            .collect();
        associations.sort_by(|left, right| {
            left.assistant_association_id
                .cmp(&right.assistant_association_id)
        });
        associations
    }

    /// Persist a new association under its (already resolved) parent assistant
    /// and index it by ARN. `false` — nothing stored — when that assistant
    /// already holds [`MAX_ASSOCIATIONS_PER_ASSISTANT`], so the bound holds
    /// however the caller reaches this.
    pub fn insert_assistant_association(&mut self, association: AssistantAssociation) -> bool {
        if self
            .assistant_associations_of(&association.assistant_id)
            .len()
            >= MAX_ASSOCIATIONS_PER_ASSISTANT
        {
            return false;
        }
        self.assistant_association_arn_index.insert(
            association.assistant_association_arn.clone(),
            (
                association.assistant_id.clone(),
                association.assistant_association_id.clone(),
            ),
        );
        self.assistant_associations.insert(
            association.assistant_association_id.clone(),
            association,
        );
        true
    }

    /// Drop one association and every index entry that named it — including the
    /// `clientToken` entry that created it, so a replay of that token does not
    /// resurrect a resource a delete removed.
    pub fn remove_assistant_association(
        &mut self,
        assistant_label: &str,
        association_label: &str,
    ) -> Option<AssistantAssociation> {
        let id = self
            .assistant_association(assistant_label, association_label)?
            .assistant_association_id
            .clone();
        let association = self.assistant_associations.remove(&id)?;
        self.assistant_association_arn_index
            .remove(&association.assistant_association_arn);
        self.idempotency.retain(|_, resource_id| *resource_id != id);
        self.idempotency_order
            .retain(|key| self.idempotency.contains_key(key));
        Some(association)
    }

    /// The listing a page starts at: the creation-ordered assistants at or
    /// after `next_token`'s cursor, or the whole listing when the caller named
    /// no token. `None` means the token is not a cursor this listing issues,
    /// which is a refusal; a cursor past every entry is simply an empty last
    /// page, because the assistants it named were deleted.
    pub fn listing_from(&self, next_token: Option<&str>) -> Option<&[ListingEntry]> {
        let Some(token) = next_token else {
            return Some(&self.assistant_order);
        };
        let cursor: u64 = token.parse().ok()?;
        // A cursor this listing never issued is not a token of this listing,
        // even when it parses: every issued token is the cursor of an entry that
        // existed, so a cursor at or past the next one to be minted is refused
        // rather than answered with an empty success a client reads as "the end".
        if cursor >= self.next_cursor {
            return None;
        }
        let start = self
            .assistant_order
            .iter()
            .position(|entry| entry.cursor >= cursor)
            .unwrap_or(self.assistant_order.len());
        Some(&self.assistant_order[start..])
    }

    /// The resource id a previous call created for this
    /// (operation, scope, `clientToken`). Whether that resource still exists is
    /// the caller's check: a token whose resource was deleted has its entry
    /// dropped by [`Self::remove_assistant`], so it replays as if unused.
    pub fn replayed_resource_id(
        &self,
        operation: &str,
        scope: &str,
        client_token: &str,
    ) -> Option<&String> {
        self.idempotency.get(&IdempotencyKey {
            operation: operation.to_string(),
            scope: scope.to_string(),
            client_token: client_token.to_string(),
        })
    }

    /// Record which resource a `clientToken` created, so a retry of the same
    /// create returns that resource instead of a duplicate.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        scope: &str,
        client_token: &str,
        resource_id: &str,
    ) {
        let key = IdempotencyKey {
            operation: operation.to_string(),
            scope: scope.to_string(),
            client_token: client_token.to_string(),
        };
        if let Some(known) = self.idempotency.get_mut(&key) {
            // Re-remembering a token already held evicts nothing: it names the
            // same entry.
            *known = resource_id.to_string();
            return;
        }
        // The index is bounded: once it is full the token remembered longest
        // ago is forgotten, so a token stream cannot grow this map forever.
        while self.idempotency_order.len() >= MAX_REMEMBERED_CLIENT_TOKENS {
            match self.idempotency_order.pop_front() {
                Some(oldest) => {
                    self.idempotency.remove(&oldest);
                }
                None => break,
            }
        }
        self.idempotency.insert(key.clone(), resource_id.to_string());
        self.idempotency_order.push_back(key);
    }
}

/// A fresh resource id: a lowercase hyphenated v4 uuid, the form the model's
/// `UuidOrArn` pattern requires (`^[a-f0-9]{8}-[a-f0-9]{4}-…$`).
///
/// Minted by the framework's own v4 generator, `emulator_telemetry`'s
/// `random()` id constructor ("A fresh random (v4) identifier"), which this
/// crate already depends on — so the ids come from the `uuid` crate the
/// framework uses, and this service declares no id generator and no RNG
/// dependency of its own.
pub fn new_resource_id() -> String {
    ::emulator_telemetry::SessionId::random().as_str().to_string()
}

/// The region a request was signed for, read from its SigV4 credential scope
/// (`Credential=<akid>/<date>/<region>/wisdom/aws4_request`), falling back to
/// [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an assistant: `arn:aws:wisdom:<region>:<account>:assistant/<id>`.
pub fn assistant_arn(region: &str, assistant_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("assistant/{assistant_id}"))
}

/// The ARN of a knowledge base:
/// `arn:aws:wisdom:<region>:<account>:knowledge-base/<knowledgeBaseId>`.
pub fn knowledge_base_arn(region: &str, knowledge_base_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("knowledge-base/{knowledge_base_id}"),
    )
}

/// The ARN of a session: `arn:aws:wisdom:<region>:<account>:session/<assistantId>/<sessionId>`.
///
/// The session's own id is the LAST segment and the owning assistant's the
/// middle one, which is the only two-segment form the model's `UuidOrArn`
/// pattern accepts (`<label>/<uuid>(/<uuid>)?`).
pub fn session_arn(region: &str, assistant_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("session/{assistant_id}/{session_id}"),
    )
}

/// The ARN of an assistant association:
/// `arn:aws:wisdom:<region>:<account>:association/<assistantId>/<assistantAssociationId>`.
///
/// The association's own id is the LAST segment and the owning assistant's the
/// middle one, the same two-segment form [`session_arn`] uses and the only one
/// the model's `Arn` pattern accepts for a nested resource.
pub fn assistant_association_arn(
    region: &str,
    assistant_id: &str,
    assistant_association_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("association/{assistant_id}/{assistant_association_id}"),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWisdomWisdomserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWisdomWisdomserviceState>)
}
