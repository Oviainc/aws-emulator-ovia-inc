// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for sharing a custom lens: `CreateLensShare`, and the share
//! invitation the share raises, which `ListShareInvitations` reports.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. A share GRANTS access to a PUBLISHED custom lens to a principal that does not have
//!    it: an accepted request answers a minted `ShareId` matching `^[0-9a-f]{32}$`,
//!    unique across shares and not derived from the request, and persists the share with
//!    the principal it was offered to, the lens's owning account as its sharer, its own
//!    `ShareInvitationId` and the `PENDING` status a share waits on its invitation in.
//!    Rows cross the three kinds of principal `SharedWith` publishes with several lenses.
//! 2. A lens that cannot be shared refuses the share and records nothing: an Amazon Web
//!    Services official lens is a 400 (some operations "are not permitted on Amazon Web
//!    Services official lenses" — the `LensAlias` doc), a `LensAlias` naming no stored
//!    custom lens is a 404, a lens no version has been published from is a 400 ("The lens
//!    cannot be applied to workloads or shared with other Amazon Web Services accounts
//!    until it's published with CreateLensVersion" — the `ImportLens` doc), and so is a
//!    `SharedWith` naming the principal that is doing the sharing.
//! 3. A share belongs to (lens, principal): the same principal cannot hold two shares of
//!    one lens (`409 ConflictException`), an `@idempotencyToken` replay returns the
//!    original share rather than minting a second, and the same token with different
//!    parameters is a 409.
//! 4. Shares are metered per lens: the share past this emulator's ceiling is the modelled
//!    `ServiceQuotaExceededException` (402) and records nothing.
//! 5. The invitation a lens share raises is a `LENS` invitation: it carries the shared
//!    lens's ARN and name and none of the members belonging to an invitation of another
//!    resource kind, it narrows by `ShareResourceType` and by `LensNamePrefix`, and
//!    reading it changes nothing a later caller sees.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`; the import,
//! document and publish helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell.
//!
//! What a share records is read back through the store, because `CreateLensShare` is the
//! only operation that REPORTS a lens share so far: `ListLensShares` is a stub with its
//! own issue, and `DeleteLensShare` — whose own suite is `super::lens_share_deletion` —
//! answers no members. The store is read exactly as `super::custom_lens_versions` reads a
//! published version back, and the HTTP read-back of a recorded share is its idempotent
//! replay and the conflict a second share collides with.
//!
//! The share a RULE-5 invitation stands for is seeded through `record_lens_share` (see
//! its doc), for the reason `Client::record_workload_share` gives: an invitation is
//! offered to the principal a lens was shared WITH, and a share whose recipient is the
//! caller has no HTTP path in a single-account emulator.

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, member, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::lens_resolution::field_names;
// The invitation reads this suite asserts are the ones `share_invitations` already
// exercises for a workload share, so its helpers are the ones used here rather than a
// second copy: `member` reads a member of ONE summary (the `custom_lens_import::member`
// above reads one of a response BODY, hence the alias).
use super::share_invitations::{invitations, member as summary_member};
use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// Principals a lens can be shared WITH: "The Amazon Web Services account ID,
/// organization ID, or organizational unit (OU) ID with which the workload, lens, profile,
/// or review template is shared" (`SharedWith` doc, `@length(12, 2048)` and no pattern),
/// so the rows span all three kinds rather than only an account id.
const PRINCIPALS: [(&str, &str); 5] = [
    ("another-account", "111122223333"),
    ("a-third-account", "444455556666"),
    ("an-account-of-nines", "999999999999"),
    ("an-organization", "o-abcd1234efgh"),
    ("an-organizational-unit", "ou-abcd-1a2b3c4d"),
];

/// One share as the store holds it — what `CreateLensShare` recorded. `pub(crate)` because
/// `super::lens_share_deletion` asserts over the same stored shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredShare {
    pub(crate) share_id: String,
    pub(crate) shared_with: String,
    pub(crate) status: String,
    pub(crate) shared_by: String,
    pub(crate) invitation_id: String,
    pub(crate) lens_arn: String,
}

/// `POST /lenses/{LensAlias}/shares`, the wire form of `CreateLensShare`. The alias is
/// percent-encoded because a custom lens is named by its ARN, which carries `:` and `/`
/// inside ONE path segment. `pub(crate)` because `super::lens_share_deletion` grants the
/// shares it revokes through this one spelling of the request.
pub(crate) async fn create_share(
    client: &Client,
    alias: &str,
    shared_with: &str,
    token: &str,
) -> Answer {
    client
        .send(
            "POST",
            &format!("/lenses/{}/shares", encode_path_segment(alias)),
            Some(json!({ "SharedWith": shared_with, "ClientRequestToken": token })),
        )
        .await
}

/// The `ShareId` a successful `CreateLensShare` answered with, checked against the
/// modelled `^[0-9a-f]{32}$` pattern.
pub(crate) fn created_share_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateLensShare 200, got {answer:?}"
    );
    let id = member(answer, "ShareId").to_string();
    assert!(
        is_hex_id(&id),
        "case {case:?}: ShareId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}

fn is_hex_id(value: &str) -> bool {
    value.len() == 32 && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// The lens documents the suites below share, as `(case, document)`: the rows differ along
/// the SHAPE of the lens being shared, because the lens is the resource the share is of,
/// and along its NAME, which is what a `LensNamePrefix` narrows.
fn lens_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal",
            lens_document("Shared-minimal-lens", None, &[("governance", 1, 2)]),
        ),
        (
            "described",
            lens_document(
                "Shared-described-lens",
                Some("the questions company policy adds to a review"),
                &[("security", 2, 3)],
            ),
        ),
        (
            "broad",
            lens_document(
                "Shared-broad-lens",
                None,
                &[("operations", 1, 1), ("cost", 2, 4)],
            ),
        ),
    ]
}

/// The `name` of a lens document.
fn document_name(document: &Value) -> String {
    document
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Import every document, wait once for the imports to land and publish a version of each
/// — a lens can only be shared once a version of it is published. Answers
/// `(case, lens ARN)` per row, in the order the rows were given.
pub(crate) async fn publish_lenses(
    client: &Client,
    cases: &[(&'static str, Value)],
) -> Vec<(&'static str, String)> {
    let mut lenses: Vec<(&'static str, String)> = Vec::new();
    for (case, document) in cases {
        let answer = import(
            client,
            &document.to_string(),
            None,
            &format!("import-{case}"),
            None,
        )
        .await;
        lenses.push((*case, imported_arn(&answer, case)));
    }
    wait_for_the_import_to_land();
    for (case, arn) in &lenses {
        let published =
            create_version(client, arn, "1.0", Some(true), &format!("publish-{case}")).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected CreateLensVersion 200, got {published:?}"
        );
    }
    lenses
}

/// One published lens, for a suite that needs a single lens to share.
pub(crate) async fn one_published_lens(client: &Client, case: &'static str, name: &str) -> String {
    let document = lens_document(name, None, &[("governance", 1, 2)]);
    let mut lenses = publish_lenses(client, &[(case, document)]).await;
    lenses
        .pop()
        .map(|(_, arn)| arn)
        .expect("publish_lenses answers one lens per row")
}

/// Every share the store holds for the lens `alias` names, in `ShareId` order.
///
/// `CreateLensShare` is the only operation that reaches one, and nothing over the wire
/// lists them yet (`ListLensShares` is a stub with its own issue), so the stored record is
/// read here — exactly as `custom_lens_versions` reads back a published lens version.
pub(crate) fn stored_shares(client: &Client, alias: &str) -> Vec<StoredShare> {
    let store = client.store();
    let store = store.lock();
    store
        .lens_shares_of(state::lens_alias(alias))
        .into_iter()
        .map(|share| StoredShare {
            share_id: share.share_id.clone(),
            shared_with: share.shared_with.clone(),
            status: share.status.clone(),
            shared_by: share.shared_by.clone(),
            invitation_id: share.share_invitation_id.clone(),
            lens_arn: share.lens_arn.clone(),
        })
        .collect()
}

/// Record a share of the lens `alias` with `shared_with` straight through the shared
/// state, and answer its `(ShareId, ShareInvitationId)`.
///
/// The lens counterpart of `Client::record_workload_share`, for the same reason: a share
/// invitation is offered to the principal a lens was shared WITH, so a test of the
/// invitation reads needs a share whose recipient is the CALLER — and that share has no
/// HTTP path here, because `CreateLensShare` refuses a `SharedWith` naming the principal
/// doing the sharing and this emulator attributes every request to its one account. The
/// seed records exactly what `CreateLensShare` records (rule 1 asserts that), and the READ
/// under test still crosses the HTTP boundary.
fn record_lens_share(client: &Client, alias: &str, shared_with: &str) -> (String, String) {
    let state_handle = client.store();
    let mut store = state_handle.lock();
    let (lens_id, lens_arn, shared_by) = store
        .lens_by_alias(alias)
        .map(|record| {
            (
                record.lens_id.clone(),
                record.lens_arn.clone(),
                record.owner.clone(),
            )
        })
        .expect("the lens under test must be stored");
    let share_id = store.mint_share_id();
    let share_invitation_id = store.mint_share_invitation_id();
    store
        .lens_shares
        .entry(lens_id.clone())
        .or_default()
        .insert(
            share_id.clone(),
            state::LensShareRecord {
                share_id: share_id.clone(),
                share_invitation_id: share_invitation_id.clone(),
                lens_id,
                lens_arn,
                shared_by,
                shared_with: shared_with.to_string(),
                status: state::DEFAULT_SHARE_STATUS.to_string(),
                status_message: None,
            },
        );
    (share_id, share_invitation_id)
}

/// Rule 1: every kind of principal, on every shape of published lens, gets a share that
/// is minted, unique and persisted with the members a share carries — and the replay of
/// the create reads that same share back over HTTP.
#[tokio::test]
async fn sharing_a_published_lens_mints_and_persists_one_share_per_principal() {
    let client = Client::new();
    let owner = engine::emulator_account_id();
    let lenses = publish_lenses(&client, &lens_cases()).await;

    let mut minted: Vec<String> = Vec::new();
    for (lens_case, arn) in &lenses {
        for (principal_case, shared_with) in PRINCIPALS {
            let case = format!("{lens_case}/{principal_case}");
            let token = format!("tok-{lens_case}-{principal_case}");
            let created = create_share(&client, arn, shared_with, &token).await;
            let share_id = created_share_id(&created, &case);

            let stored = stored_shares(&client, arn);
            let found = stored
                .iter()
                .find(|share| share.share_id == share_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the share must be persisted, got {stored:?}")
                });
            assert_eq!(
                found.shared_with, shared_with,
                "case {case:?}: the share must name the principal it was offered to, got \
                 {found:?}"
            );
            assert_eq!(
                found.status,
                state::DEFAULT_SHARE_STATUS,
                "case {case:?}: a new share waits on its invitation, got {found:?}"
            );
            assert_eq!(
                found.shared_by, owner,
                "case {case:?}: the lens's owning account is the sharer, got {found:?}"
            );
            assert_eq!(
                &found.lens_arn, arn,
                "case {case:?}: the share must belong to the lens it shares, got {found:?}"
            );
            assert!(
                is_hex_id(&found.invitation_id),
                "case {case:?}: ShareInvitationId must match ^[0-9a-f]{{32}}$, got {found:?}"
            );
            assert_ne!(
                found.invitation_id, share_id,
                "case {case:?}: the invitation is its own resource, so its id is not the \
                 ShareId, got {found:?}"
            );
            assert!(
                !share_id.contains(shared_with),
                "case {case:?}: the ShareId must not be derived from the request, got \
                 {share_id:?}"
            );

            // The replay of the create is the read-back over HTTP: the same token with the
            // same parameters reports the share that was recorded, and mints no second one.
            let replayed = create_share(&client, arn, shared_with, &token).await;
            assert_eq!(
                created_share_id(&replayed, &case),
                share_id,
                "case {case:?}: an idempotent replay must report the original share, got \
                 {replayed:?}"
            );
            minted.push(share_id);
        }
    }

    assert_eq!(
        minted.len(),
        lenses.len() * PRINCIPALS.len(),
        "every row must have produced one share, got {minted:?}"
    );
    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "no two shares may carry the same ShareId, got {minted:?}"
    );
    for (lens_case, arn) in &lenses {
        let stored = stored_shares(&client, arn);
        assert_eq!(
            stored.len(),
            PRINCIPALS.len(),
            "case {lens_case:?}: the lens must hold one share per principal, got {stored:?}"
        );
    }
}

/// Rule 2: a lens that cannot be shared, and a principal that cannot be shared with, are
/// refused and record nothing. The rows span both spellings of an Amazon Web Services
/// official lens, a well-formed custom ARN naming no lens, a landed import nothing was
/// published from, an import still in flight, and the sharer's own principal.
#[tokio::test]
async fn a_lens_that_cannot_be_shared_refuses_the_share_and_records_nothing() {
    let client = Client::new();
    let owner = engine::emulator_account_id();

    // A lens whose import has landed but which no `CreateLensVersion` has published.
    let draft = import(
        &client,
        &lens_document("Draft-lens", None, &[("governance", 1, 2)]).to_string(),
        None,
        "import-draft",
        None,
    )
    .await;
    let draft_arn = imported_arn(&draft, "draft");
    wait_for_the_import_to_land();

    // A lens whose FIRST import has not landed yet: nothing is published from it either,
    // so the same rule refuses it.
    let in_flight = import(
        &client,
        &lens_document("In-flight-lens", None, &[("security", 1, 2)]).to_string(),
        None,
        "import-in-flight",
        None,
    )
    .await;
    let in_flight_arn = imported_arn(&in_flight, "in-flight");

    let published_arn = one_published_lens(&client, "shareable", "Shareable-lens").await;

    // `(case, LensAlias, SharedWith, expected status, expected error code)`.
    let rows: Vec<(&str, String, &str, u16, &str)> = vec![
        (
            "official-bare-alias",
            state::FRAMEWORK_LENS_ALIAS.to_string(),
            "111122223333",
            400,
            "ValidationException",
        ),
        (
            "official-lens-arn",
            format!(
                "arn:aws:wellarchitected:us-west-2::lens/{}",
                state::FRAMEWORK_LENS_ALIAS
            ),
            "111122223333",
            400,
            "ValidationException",
        ),
        (
            "another-official-lens",
            "serverless".to_string(),
            "111122223333",
            400,
            "ValidationException",
        ),
        (
            "unknown-custom-lens",
            ABSENT_LENS_ARN.to_string(),
            "111122223333",
            404,
            "ResourceNotFoundException",
        ),
        (
            "draft-lens",
            draft_arn.clone(),
            "111122223333",
            400,
            "ValidationException",
        ),
        (
            "import-still-in-flight",
            in_flight_arn.clone(),
            "111122223333",
            400,
            "ValidationException",
        ),
        (
            "shared-with-the-sharer",
            published_arn.clone(),
            owner,
            400,
            "ValidationException",
        ),
    ];

    for (case, alias, shared_with, status, error_code) in &rows {
        let refused = create_share(&client, alias, shared_with, &format!("tok-{case}")).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, *error_code,
            "case {case:?}: expected {error_code}, got {refused:?}"
        );
    }

    for (case, alias) in [
        ("draft-lens", &draft_arn),
        ("import-still-in-flight", &in_flight_arn),
        ("shared-with-the-sharer", &published_arn),
    ] {
        let stored = stored_shares(&client, alias);
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused share must record nothing, got {stored:?}"
        );
    }

    // The refusal that is about the LENS names the member that named it, so a client can
    // tell it from the one about the principal.
    let refused_lens = create_share(
        &client,
        state::FRAMEWORK_LENS_ALIAS,
        "111122223333",
        "tok-official-fields",
    )
    .await;
    assert!(
        field_names(&refused_lens).contains(&"LensAlias".to_string()),
        "case \"official-bare-alias\": the refusal must name LensAlias, got {refused_lens:?}"
    );
    let refused_principal =
        create_share(&client, &published_arn, owner, "tok-self-share-fields").await;
    assert!(
        field_names(&refused_principal).contains(&"SharedWith".to_string()),
        "case \"shared-with-the-sharer\": the refusal must name SharedWith, got \
         {refused_principal:?}"
    );

    // And a lens that CAN be shared still is, so the refusals above are about their rows
    // and not about every request.
    let accepted = create_share(&client, &published_arn, "111122223333", "tok-accepted").await;
    let share_id = created_share_id(&accepted, "accepted");
    let stored = stored_shares(&client, &published_arn);
    assert_eq!(
        stored.len(),
        1,
        "case \"accepted\": the accepted share is the only one recorded, got {stored:?}"
    );
    assert_eq!(
        stored[0].share_id, share_id,
        "case \"accepted\": the recorded share must be the one answered, got {stored:?}"
    );
}

/// Rule 3: one principal holds at most one share of one lens, and the idempotency token
/// decides whether a repeat is a replay or a collision. The rows span the three kinds of
/// principal.
#[tokio::test]
async fn one_principal_holds_at_most_one_share_of_one_lens() {
    let client = Client::new();
    let arn = one_published_lens(&client, "conflicts", "Conflicting-shares-lens").await;

    for (case, shared_with) in PRINCIPALS {
        let token = format!("tok-{case}");
        let first = create_share(&client, &arn, shared_with, &token).await;
        let share_id = created_share_id(&first, case);

        // A second share of the same lens for the same principal, under a FRESH token.
        let again = create_share(&client, &arn, shared_with, &format!("{token}-again")).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: a second share for one principal must be a 409, got {again:?}"
        );
        assert_eq!(
            again.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {again:?}"
        );
        assert_eq!(
            member(&again, "ResourceId"),
            share_id,
            "case {case:?}: the conflict must name the share that is in the way, got {again:?}"
        );

        // The SAME token with DIFFERENT parameters is not a replay.
        let different = create_share(&client, &arn, "777788889999", &token).await;
        assert_eq!(
            different.status, 409,
            "case {case:?}: a token reused with other parameters must be a 409, got \
             {different:?}"
        );
        assert_eq!(
            different.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {different:?}"
        );
    }

    let stored = stored_shares(&client, &arn);
    assert_eq!(
        stored.len(),
        PRINCIPALS.len(),
        "every principal must hold exactly one share, got {stored:?}"
    );
}

/// Rule 4: the share past the per-lens ceiling is the modelled
/// `ServiceQuotaExceededException` and records nothing, while the share AT the ceiling is
/// still served.
///
/// The lens is filled to one share below the ceiling through the shared state, because
/// reaching it over HTTP would take a thousand signed requests; the two boundary rows
/// themselves are crossed over HTTP.
#[tokio::test]
async fn the_share_past_the_per_lens_ceiling_is_refused() {
    let client = Client::new();
    let arn = one_published_lens(&client, "quota", "Quota-lens").await;

    for index in 0..state::MAX_SHARES_PER_LENS - 1 {
        record_lens_share(&client, &arn, &format!("{index:012}"));
    }
    assert_eq!(
        stored_shares(&client, &arn).len(),
        state::MAX_SHARES_PER_LENS - 1,
        "the lens must be seeded to one share below the ceiling"
    );

    let last = create_share(&client, &arn, "111122223333", "tok-at-the-ceiling").await;
    created_share_id(&last, "at-the-ceiling");

    let past = create_share(&client, &arn, "444455556666", "tok-past-the-ceiling").await;
    assert_eq!(
        past.status, 402,
        "case \"past-the-ceiling\": expected HTTP 402, got {past:?}"
    );
    assert_eq!(
        past.error_code, "ServiceQuotaExceededException",
        "case \"past-the-ceiling\": expected ServiceQuotaExceededException, got {past:?}"
    );
    let stored = stored_shares(&client, &arn);
    assert_eq!(
        stored.len(),
        state::MAX_SHARES_PER_LENS,
        "a refused share must record nothing, so the lens still holds exactly {} shares",
        state::MAX_SHARES_PER_LENS
    );
    assert!(
        !stored
            .iter()
            .any(|share| share.shared_with == "444455556666"),
        "case \"past-the-ceiling\": the refused principal must hold no share"
    );
}

/// Rule 5: the invitation a lens share raises is a `LENS` invitation naming the shared
/// lens, it narrows by `ShareResourceType` and `LensNamePrefix`, and reading it mutates
/// nothing. The lens rows differ along the lens's name, so the prefixes select different
/// counts.
#[tokio::test]
async fn a_lens_share_raises_a_lens_invitation_for_the_principal_it_was_offered_to() {
    let client = Client::new();
    let caller = engine::emulator_account_id();
    let cases = lens_cases();
    let lenses = publish_lenses(&client, &cases).await;

    // One invitation per lens, addressed to the caller, plus one addressed elsewhere —
    // which is none of this caller's business and must not be reported.
    let mut offered: Vec<(&'static str, String, String)> = Vec::new();
    for (case, arn) in &lenses {
        let (share_id, invitation_id) = record_lens_share(&client, arn, caller);
        offered.push((*case, share_id, invitation_id));
    }
    let elsewhere = create_share(
        &client,
        &lenses[0].1,
        "111122223333",
        "tok-offered-elsewhere",
    )
    .await;
    created_share_id(&elsewhere, "offered-elsewhere");

    let listed = client.list_share_invitations("ShareResourceType=LENS").await;
    let reported = invitations(&listed, "lens-invitations");
    assert_eq!(
        reported.len(),
        lenses.len(),
        "case \"lens-invitations\": expected one invitation per shared lens, got {listed:?}"
    );

    for (index, (case, share_id, invitation_id)) in offered.iter().enumerate() {
        let found = reported
            .iter()
            .find(|invitation| summary_member(invitation, "ShareInvitationId") == invitation_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the pending share must be reported, got {listed:?}")
            });
        assert_eq!(
            summary_member(found, "ShareResourceType"),
            "LENS",
            "case {case:?}: a share of a lens is a LENS invitation, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "LensArn"),
            lenses[index].1,
            "case {case:?}: the invitation must name the shared lens, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "LensName"),
            document_name(&cases[index].1),
            "case {case:?}: the invitation must carry the lens's name, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "SharedWith"),
            caller,
            "case {case:?}: the invitation is the one offered to the caller, got {found:?}"
        );
        assert_eq!(
            summary_member(found, "SharedBy"),
            caller,
            "case {case:?}: the invitation must name the account that shared the lens, got \
             {found:?}"
        );
        assert_ne!(
            summary_member(found, "ShareInvitationId"),
            share_id.as_str(),
            "case {case:?}: the invitation is its own resource, so its id is not the ShareId, \
             got {found:?}"
        );
        // A lens share grants no permission, and the members of another resource kind
        // belong to that kind.
        for absent in [
            "PermissionType",
            "WorkloadId",
            "WorkloadName",
            "ProfileName",
            "ProfileArn",
            "TemplateName",
            "TemplateArn",
        ] {
            assert_eq!(
                found.get(absent),
                None,
                "case {case:?}: a lens invitation carries no {absent}, got {found:?}"
            );
        }
    }

    // `LensNamePrefix` narrows by the lens's NAME; the rows differ along the prefix's
    // length and along how many of the shared lenses it selects.
    for (case, prefix, expected) in [
        ("every-lens", "Shared-", lenses.len()),
        ("one-letter", "S", lenses.len()),
        ("one-lens", "Shared-b", 1),
        ("another-lens", "Shared-d", 1),
        ("exact-name", "Shared-minimal-lens", 1),
        ("no-lens", "zzz", 0),
    ] {
        let narrowed = client
            .list_share_invitations(&format!("LensNamePrefix={prefix}"))
            .await;
        let found = invitations(&narrowed, case);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} invitation(s) for prefix {prefix:?}, got \
             {narrowed:?}"
        );
        for invitation in &found {
            let name = summary_member(invitation, "LensName");
            assert!(
                name.starts_with(prefix),
                "case {case:?}: {name:?} does not start with {prefix:?}, got {narrowed:?}"
            );
        }
    }

    // A workload prefix selects invitations of that kind, of which there are none here.
    let crossed = client
        .list_share_invitations("WorkloadNamePrefix=Shared-")
        .await;
    assert_eq!(
        invitations(&crossed, "a-workload-prefix").len(),
        0,
        "case \"a-workload-prefix\": a lens invitation carries no workload name, got {crossed:?}"
    );

    // Reading does not mutate: a second read reports the same invitations, and the shares
    // they stand for still wait on them.
    let again = client.list_share_invitations("ShareResourceType=LENS").await;
    assert_eq!(
        invitations(&again, "repeat-read"),
        reported,
        "case \"repeat-read\": a read must not change what the next caller sees, got \
         {listed:?} then {again:?}"
    );
    for (case, arn) in &lenses {
        for share in stored_shares(&client, arn) {
            assert_eq!(
                share.status,
                state::DEFAULT_SHARE_STATUS,
                "case {case:?}: reading the invitations must not move a share's status, got \
                 {share:?}"
            );
        }
    }
}
