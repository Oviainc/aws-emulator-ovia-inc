// SPDX-License-Identifier: CC0-1.0

//! WHERE a refusal of the key group surface takes its MESSAGE from, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test, over every modelled refusal this surface serves:
//!
//! * a refusal carries the sentence the SERVICE answers that error class with at runtime, and
//!   every entry point into one condition answers the SAME sentence — the create and the
//!   update that moves a name onto a group reach one name-uniqueness rule, and the four calls
//!   that address a group by id reach one absent-group rule, so each set reads alike;
//! * the `documentation` of the error shape is prose ABOUT the condition and is not a source
//!   of response text: where the two differ, what a client reads is the service's sentence and
//!   never the documented one. Where the service's own sentence IS the documented one — the
//!   published key-quota refusal below — the row records that, so this is a rule about where
//!   the text comes FROM rather than a claim that the two always differ.
//!
//! The rows span what the rule generalises over: the error CLASS (six of them, four HTTP
//! statuses between them), the entry POINT that refuses (the create, the update, both reads
//! and the delete — five operations), and for each pair two payload variants, so the sentence
//! cannot depend on the configuration submitted, on how many public keys it names, on the
//! form the unusable version token took or on which unheld identifier was addressed.
//!
//! Both sentences of every row are LITERALS below rather than the crate's own constants, so a
//! wrong constant cannot make this suite pass; the documented ones are the model's own, with
//! the markup stripped
//! (`smithy_query(query="shape",shape="KeyGroupAlreadyExists,NoSuchResource,PreconditionFailed,InvalidIfMatchVersion,InvalidArgument,TooManyPublicKeysInKeyGroup",model_path="services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json")`).
//!
//! The harness (the router, the request helpers, the public key minting and the key group
//! configuration document) is the key group lifecycle and distribution lifecycle suites',
//! reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_PUBLIC_KEYS_IN_KEY_GROUP;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::key_group_lifecycle::{create_group, get_group, mint_public_keys, Group};

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/key-group/{Id}` with the `KeyGroupConfig` document `body`, carrying
/// `If-Match` exactly as `if_match` gives it (left off entirely when it is `None`).
async fn update_group(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/key-group/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// `GET /2020-05-31/key-group/{Id}/config` — the second reader of the one stored record.
async fn get_group_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/key-group/{id}/config"),
        None,
        None,
    )
    .await
}

/// `DELETE /2020-05-31/key-group/{Id}` under the version token `if_match`.
async fn delete_group(router: &MultiServiceRouter, id: &str, if_match: Option<&str>) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/key-group/{id}"),
        None,
        if_match,
    )
    .await
}

// ---------------------------------------------------------------------------
// The refusals under test
// ---------------------------------------------------------------------------

/// One operation of the key group surface that can refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Create,
    Update,
    Get,
    GetConfig,
    Delete,
}

/// One condition this surface refuses, whichever operation runs into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Condition {
    /// A `KeyGroupConfig$Name` another stored group already holds.
    DuplicateName,
    /// An id, well formed for this kind, that no stored group holds.
    UnheldId,
    /// An `If-Match` that is absent or is no version token at all.
    UnusableIfMatch,
    /// An `If-Match` carrying a version token that is not this group's current one.
    StaleIfMatch,
    /// A configuration that names no public key at all.
    NoPublicKey,
    /// A configuration naming more public keys than the published quota on one group's list.
    PastKeyQuota,
}

/// One error class: the operations that reach it, the refusal it travels as, the sentence the
/// service answers it with and the model's documentation of the error shape.
struct Row {
    condition: Condition,
    /// The entry points into this condition — every one of them answers the same sentence.
    entries: &'static [Entry],
    status: u16,
    code: &'static str,
    /// The sentence the SERVICE answers this class with, as a literal.
    served: &'static str,
    /// The `documentation` of the error shape the class travels as, markup stripped, as a
    /// literal. Prose about the condition: never what a client reads, unless the service's
    /// own sentence happens to be this one (`documented_is_served` below).
    documented: &'static str,
}

impl Row {
    /// Whether this class's runtime sentence IS the documented one, so the row asserts the
    /// two coincide rather than asserting they differ.
    fn documented_is_served(&self) -> bool {
        self.served == self.documented
    }
}

/// How many payload variants each (condition, entry) pair is driven over.
const VARIANTS: usize = 2;

/// Every refusal of this surface under test.
const ROWS: &[Row] = &[
    Row {
        condition: Condition::DuplicateName,
        entries: &[Entry::Create, Entry::Update],
        status: 409,
        code: "KeyGroupAlreadyExists",
        served: "Another key group with the same name already exists within the aws account.",
        documented: "A key group with this name already exists. You must provide a unique name. \
                     To modify an existing key group, use UpdateKeyGroup.",
    },
    Row {
        condition: Condition::UnheldId,
        entries: &[Entry::Get, Entry::GetConfig, Entry::Update, Entry::Delete],
        status: 404,
        code: "NoSuchResource",
        served: "The specified key group does not exist.",
        documented: "A resource that was specified is not valid.",
    },
    Row {
        condition: Condition::UnusableIfMatch,
        entries: &[Entry::Update, Entry::Delete],
        status: 400,
        code: "InvalidIfMatchVersion",
        served: "The If-Match version is missing or not valid for the resource.",
        documented: "The If-Match version is missing or not valid.",
    },
    Row {
        condition: Condition::StaleIfMatch,
        entries: &[Entry::Update, Entry::Delete],
        status: 412,
        code: "PreconditionFailed",
        served: "The request failed because it didn't meet the preconditions for this operation.",
        documented: "The precondition in one or more of the request fields evaluated to false.",
    },
    Row {
        condition: Condition::NoPublicKey,
        entries: &[Entry::Create, Entry::Update],
        status: 400,
        code: "InvalidArgument",
        served: "An argument is invalid: to create a key group, you must specify at least one \
                 public key for the key group",
        documented: "An argument is invalid.",
    },
    Row {
        // The one class below whose runtime sentence is the documented one: the quota refusal
        // states the published limit, which is what the model documents too.
        condition: Condition::PastKeyQuota,
        entries: &[Entry::Create, Entry::Update],
        status: 400,
        code: "TooManyPublicKeysInKeyGroup",
        served: "The number of public keys in this key group is more than the maximum allowed. \
                 For more information, see Quotas (formerly known as limits) in the Amazon \
                 CloudFront Developer Guide.",
        documented: "The number of public keys in this key group is more than the maximum \
                     allowed. For more information, see Quotas (formerly known as limits) in \
                     the Amazon CloudFront Developer Guide.",
    },
];

/// Ids no key group holds, each in the shape this service mints a key group id in (8-4-4-4-12
/// lowercase hex), so a call addressing one runs into the refusal about the STORE rather than
/// the one about the shape of the value.
const UNHELD_IDS: [&str; VARIANTS] = [
    "c1f2a3b4-5d6e-4f70-8192-a3b4c5d6e7f8",
    "0a1b2c3d-4e5f-4061-8273-849506a7b8c9",
];

/// The forms an `If-Match` that is no version token at all arrives in: left off the request
/// entirely, and a value outside the shape a version token takes.
const UNUSABLE_IF_MATCH: [Option<&str>; VARIANTS] = [None, Some("not-a-version-token")];

/// A version token that is well formed and that this service never minted, for the stale
/// precondition the other variant drives with another group's current token.
const NEVER_MINTED_TOKEN: &str = "E2STALEVERSION";

// ---------------------------------------------------------------------------
// The store every refusal is driven against
// ---------------------------------------------------------------------------

/// The two key groups and the public keys the rows above are driven over.
#[derive(Debug)]
struct Fixture {
    /// Public key ids `CreatePublicKey` minted — one more than the published quota on one
    /// group's list, so the quota rows are driven with REAL key ids.
    keys: Vec<String>,
    /// The group that holds the name the duplicate-name rows try to move.
    holder: Group,
    /// The holder's current version token, which is a stale one for any other group.
    holder_etag: String,
    /// The group every refusal that addresses a stored group addresses.
    subject: Group,
    subject_id: String,
    subject_etag: String,
}

/// Create one key group through the API and report `(Id, ETag)`.
async fn create_stored(router: &MultiServiceRouter, row: &Group) -> (String, String) {
    let created = create_group(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token in ETag, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// Stand the store up: the public keys a configuration names, the group that holds the taken
/// name, and the group the refusals address.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let keys = mint_public_keys(
        router,
        "kg-message-source",
        MAX_PUBLIC_KEYS_IN_KEY_GROUP + 1,
    )
    .await;
    let holder = Group {
        case: "the group that holds the taken name".to_string(),
        name: "kg-message-source-holder".to_string(),
        items: keys[..2].to_vec(),
        comment: Some("the group whose name the rows try to take".to_string()),
    };
    let (_holder_id, holder_etag) = create_stored(router, &holder).await;
    let subject = Group {
        case: "the group every refusal addresses".to_string(),
        name: "kg-message-source-subject".to_string(),
        items: keys[..1].to_vec(),
        comment: None,
    };
    let (subject_id, subject_etag) = create_stored(router, &subject).await;
    Fixture {
        keys,
        holder,
        holder_etag,
        subject,
        subject_id,
        subject_etag,
    }
}

// ---------------------------------------------------------------------------
// The configurations the refused calls submit
// ---------------------------------------------------------------------------

/// A configuration under the name the holder group already has: one variant submits the
/// holder's whole configuration, the other this group's own keys under the taken name.
fn taken_name_config(fixture: &Fixture, variant: usize) -> Group {
    let mut config = if variant.is_multiple_of(2) {
        fixture.holder.clone()
    } else {
        Group {
            items: fixture.subject.items.clone(),
            comment: Some(format!("this group's own keys under a taken name {variant}")),
            ..fixture.holder.clone()
        }
    };
    config.case = format!("a configuration under a taken name, variant {variant}");
    config
}

/// A configuration whose name no group holds, so a call carrying it is refused — if at all —
/// for something other than the name.
fn free_name_config(fixture: &Fixture, variant: usize) -> Group {
    Group {
        case: format!("a configuration under a free name, variant {variant}"),
        name: format!("kg-message-source-free-{variant}"),
        items: fixture.keys[..=variant].to_vec(),
        comment: (variant % 2 == 1).then(|| format!("a free name {variant}")),
    }
}

/// A configuration that names no public key at all.
fn no_key_config(name: &str, variant: usize) -> Group {
    Group {
        case: format!("a configuration naming no public key, variant {variant}"),
        name: name.to_string(),
        items: Vec::new(),
        comment: (variant % 2 == 1).then(|| format!("no public key at all {variant}")),
    }
}

/// A configuration naming one public key more than the published quota, in the order the
/// variant chooses — so the refusal cannot depend on which key leads the list.
fn past_quota_config(fixture: &Fixture, name: &str, variant: usize) -> Group {
    let mut items = fixture.keys[..=MAX_PUBLIC_KEYS_IN_KEY_GROUP].to_vec();
    if variant % 2 == 1 {
        items.reverse();
    }
    Group {
        case: format!("a configuration past the published key quota, variant {variant}"),
        name: name.to_string(),
        items,
        comment: (variant % 2 == 1).then(|| format!("past the quota {variant}")),
    }
}

// ---------------------------------------------------------------------------
// Driving one (condition, entry) pair
// ---------------------------------------------------------------------------

/// Run `condition` into `entry` and report what the emulator answered.
async fn refuse(
    router: &MultiServiceRouter,
    entry: Entry,
    condition: Condition,
    variant: usize,
    fixture: &Fixture,
) -> Answer {
    let subject = fixture.subject_id.as_str();
    let current = fixture.subject_etag.as_str();
    let unheld = UNHELD_IDS[variant % UNHELD_IDS.len()];
    let stale = if variant.is_multiple_of(2) {
        fixture.holder_etag.clone()
    } else {
        NEVER_MINTED_TOKEN.to_string()
    };
    match (entry, condition) {
        (Entry::Create, Condition::DuplicateName) => {
            create_group(router, &taken_name_config(fixture, variant).xml()).await
        }
        (Entry::Create, Condition::NoPublicKey) => {
            let name = format!("kg-message-source-create-no-key-{variant}");
            create_group(router, &no_key_config(&name, variant).xml()).await
        }
        (Entry::Create, Condition::PastKeyQuota) => {
            let name = format!("kg-message-source-create-quota-{variant}");
            create_group(router, &past_quota_config(fixture, &name, variant).xml()).await
        }
        (Entry::Update, Condition::DuplicateName) => {
            let body = taken_name_config(fixture, variant).xml();
            update_group(router, subject, &body, Some(current)).await
        }
        (Entry::Update, Condition::NoPublicKey) => {
            let body = no_key_config(&fixture.subject.name, variant).xml();
            update_group(router, subject, &body, Some(current)).await
        }
        (Entry::Update, Condition::PastKeyQuota) => {
            let body = past_quota_config(fixture, &fixture.subject.name, variant).xml();
            update_group(router, subject, &body, Some(current)).await
        }
        (Entry::Update, Condition::UnheldId) => {
            let body = free_name_config(fixture, variant).xml();
            update_group(router, unheld, &body, Some(current)).await
        }
        (Entry::Update, Condition::UnusableIfMatch) => {
            let body = free_name_config(fixture, variant).xml();
            let if_match = UNUSABLE_IF_MATCH[variant % UNUSABLE_IF_MATCH.len()];
            update_group(router, subject, &body, if_match).await
        }
        (Entry::Update, Condition::StaleIfMatch) => {
            let body = free_name_config(fixture, variant).xml();
            update_group(router, subject, &body, Some(&stale)).await
        }
        (Entry::Get, Condition::UnheldId) => get_group(router, unheld).await,
        (Entry::GetConfig, Condition::UnheldId) => get_group_config(router, unheld).await,
        (Entry::Delete, Condition::UnheldId) => delete_group(router, unheld, Some(current)).await,
        (Entry::Delete, Condition::UnusableIfMatch) => {
            let if_match = UNUSABLE_IF_MATCH[variant % UNUSABLE_IF_MATCH.len()];
            delete_group(router, subject, if_match).await
        }
        (Entry::Delete, Condition::StaleIfMatch) => {
            delete_group(router, subject, Some(&stale)).await
        }
        (entry, condition) => {
            panic!("case {condition:?} at {entry:?}: this pair is no refusal under test")
        }
    }
}

/// Assert that `answer` is `row`'s refusal, and report the sentence it carried.
fn assert_refusal(case: &str, answer: &Answer, row: &Row) -> String {
    assert_eq!(
        answer.status, row.status,
        "case {case}: expected HTTP {}, got {} with {}",
        row.status, answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        row.code,
        "case {case}: expected the code {}, got {:?} with {}",
        row.code,
        answer.error_code(),
        answer.raw
    );
    let message = answer
        .root()
        .find_text("Message")
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, row.served,
        "case {case}: expected the sentence {:?}, got {message:?} in {}",
        row.served, answer.raw
    );
    if !row.documented_is_served() {
        assert_ne!(
            message, row.documented,
            "case {case}: expected the service's own sentence rather than the model's \
             documentation of the error shape, got {message:?}"
        );
    }
    message
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Every modelled refusal of this surface carries the service's own sentence for its error
/// class — the same one at every entry point into the condition — and not the model's
/// documentation of the error shape.
#[tokio::test]
async fn every_refusal_carries_the_services_own_sentence_at_every_entry_point() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for row in ROWS {
        let mut answered: Vec<(String, String)> = Vec::new();
        for entry in row.entries {
            for variant in 0..VARIANTS {
                let case = format!(
                    "{:?} refused by {entry:?}, variant {variant}",
                    row.condition
                );
                let answer = refuse(&router, *entry, row.condition, variant, &fixture).await;
                let message = assert_refusal(&case, &answer, row);
                answered.push((case, message));
            }
        }

        // One condition, one sentence: the entry points into it are not worded apart.
        let (first_case, first) = answered
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: no entry point under test", row.condition));
        for (case, message) in &answered {
            assert_eq!(
                message, &first,
                "case {case}: expected the sentence {first:?} that '{first_case}' carries, got \
                 {message:?}"
            );
        }
    }

    // None of the refusals changed anything: the group they addressed still reads back as it
    // was stored, at the version token its own create minted.
    let read = get_group(&router, &fixture.subject_id).await;
    assert_eq!(
        read.status, 200,
        "case the addressed group after every refusal: expected it to be readable, got {} \
         with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        fixture.subject_etag,
        "case the addressed group after every refusal: expected version {}, got {:?}",
        fixture.subject_etag,
        read.header("etag")
    );
    fixture.subject.assert_matches(
        read.root().at("KeyGroupConfig").unwrap_or_else(|| {
            panic!("case the addressed group after every refusal: no KeyGroupConfig in {}", read.raw)
        }),
        "GetKeyGroup after every refusal",
        &read.raw,
    );

    // And the refusals were about their conditions rather than about the surface: the update
    // every row drove still lands when the condition is gone.
    let accepted = free_name_config(&fixture, 1);
    let answer = update_group(
        &router,
        &fixture.subject_id,
        &accepted.xml(),
        Some(&fixture.subject_etag),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {}: expected an update under a free name to land, got {} with {}",
        accepted.case, answer.status, answer.raw
    );
}
