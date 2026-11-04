// SPDX-License-Identifier: CC0-1.0

//! How this service reports a refusal, over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the
//! same restJson1 path a real SDK client takes, and each rule is asserted over a SET
//! of inputs rather than one request.
//!
//! The rules under test:
//!
//! * an id that names no resource of this service is `ResourceNotFoundException`
//!   carrying ONE message template, the same for every resource kind and every
//!   operation addressed by an id, whose only variable part is the id the request
//!   named; an operation addressed by a (ProfileId, ResourceId) PAIR names both ids
//!   instead, since either half can be why the pair resolves to nothing;
//! * `DeleteProfile` refuses a Profile an association still names with
//!   `ConflictException`, and the message names the blocking ASSOCIATION, read off
//!   the service's own state, so the caller is told what to delete first;
//! * a VPC admits one Profile association, so associating a VPC another Profile
//!   already holds is the quota refusal `LimitExceededException` naming the VPC,
//!   while re-associating the SAME pair reports the association as already existing;
//! * every one of these refusals is HTTP 400.

use super::harness::Emulator;

/// The single message a missing resource of this service is reported with: the
/// Resolver-family sub-error code, a fixed sentence, and the id from the request.
fn not_found_message(id: &str) -> String {
    format!("[RSLVR-05007] Can't find the resource with ID \"{id}\".")
}

/// The message an operation addressed by a (ProfileId, ResourceId) pair reports an
/// unresolvable pair with, naming both ids from the request.
fn pair_not_found_message(profile_id: &str, resource_id: &str) -> String {
    format!(
        "[RSLVR-05300] Can't locate the Profile ID: \"{profile_id}\" or resource ID \
         \"{resource_id}\"."
    )
}

/// The message `DeleteProfile` refuses a still-associated Profile with, naming the
/// association that blocks it.
fn still_associated_message(association_id: &str) -> String {
    format!(
        "[RSLVR-05102] The Profile is associated with a resource: \"{association_id}\". \
         Delete the association and try again."
    )
}

/// The message a VPC that already has a Profile is refused a second one with.
fn vpc_limit_message(resource_id: &str) -> String {
    format!("VPC [{resource_id}] exceeded its maximum profile association limit.")
}

/// Id forms differing in shape: the minted Profile form, the minted association
/// form, a bare word, a single character, and exactly the modelled 64-character
/// maximum of `ResourceId`.
fn unused_ids() -> Vec<String> {
    vec![
        "rp-0123456789abcdef".to_string(),
        "rpassoc-fedcba9876543210".to_string(),
        "nosuchresource".to_string(),
        "z".to_string(),
        "q".repeat(64),
    ]
}

#[tokio::test]
async fn a_missing_resource_is_reported_with_one_template_whatever_its_kind() {
    let em = Emulator::new();
    // A live Profile and association, so the pair-addressed operations have a half
    // that resolves and a half that does not.
    let live_profile = em
        .create_named("template-host")
        .await
        .nested_string("Profile", "Id");
    let live_vpc = "vpc-0abc1";
    em.associate(&live_profile, live_vpc, "live-assoc").await;

    for id in unused_ids() {
        // Each row is one operation addressed by a single id, with the id the refusal
        // must name: the id the request gave that resolves to nothing.
        let cases: Vec<(&str, String, _)> = vec![
            ("GetProfile", id.clone(), em.get_profile(&id).await),
            ("DeleteProfile", id.clone(), em.delete_profile(&id).await),
            (
                "GetProfileAssociation",
                id.clone(),
                em.get_association(&id).await,
            ),
            ("AssociateProfile", id.clone(), {
                em.associate(&id, "vpc-unassociated", "a1").await
            }),
        ];
        for (operation, named, reply) in cases {
            let case = format!("{operation} id={named:?}");
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {} / {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {}",
                reply.status
            );
            assert_eq!(
                reply.message(),
                not_found_message(&named),
                "case {case}: expected {:?}, got {:?}",
                not_found_message(&named),
                reply.message()
            );
        }

        // The pair-addressed operation, with each half in turn the one that resolves
        // to nothing, and with neither half resolving.
        for (profile_id, resource_id) in [
            (id.clone(), live_vpc.to_string()),
            (live_profile.clone(), id.clone()),
            (id.clone(), id.clone()),
        ] {
            let case = format!("DisassociateProfile {profile_id:?}/{resource_id:?}");
            let reply = em.disassociate(&profile_id, &resource_id).await;
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {} / {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.status, 400,
                "case {case}: expected HTTP 400, got {}",
                reply.status
            );
            assert_eq!(
                reply.message(),
                pair_not_found_message(&profile_id, &resource_id),
                "case {case}: expected {:?}, got {:?}",
                pair_not_found_message(&profile_id, &resource_id),
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_resource_that_was_removed_is_reported_with_the_same_template() {
    let em = Emulator::new();

    // Rows differing in the VPC id's form, so the removal path is exercised more than
    // once; each round deletes both a Profile and an association.
    for (index, resource_id) in ["vpc-12345678", "vpc-0a1b2c3d4e5f67890", "v"]
        .into_iter()
        .enumerate()
    {
        let profile_id = em
            .create_named(&format!("removed-{index}"))
            .await
            .nested_string("Profile", "Id");
        let association_id = em
            .associate(&profile_id, resource_id, &format!("assoc-{index}"))
            .await
            .nested_string("ProfileAssociation", "Id");

        let removed = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            removed.status, 200,
            "case resourceId={resource_id:?}: DisassociateProfile expected 200, got {} / {:?}",
            removed.status, removed.body
        );
        let gone = em.get_association(&association_id).await;
        assert_eq!(
            gone.message(),
            not_found_message(&association_id),
            "case resourceId={resource_id:?}: after DisassociateProfile, expected {:?}, got {:?}",
            not_found_message(&association_id),
            gone.message()
        );
        // Disassociating the same pair again no longer resolves it, and that refusal
        // names the pair.
        let again = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            again.message(),
            pair_not_found_message(&profile_id, resource_id),
            "case resourceId={resource_id:?}: a repeat DisassociateProfile expected {:?}, \
             got {:?}",
            pair_not_found_message(&profile_id, resource_id),
            again.message()
        );

        let deleted = em.delete_profile(&profile_id).await;
        assert_eq!(
            deleted.status, 200,
            "case resourceId={resource_id:?}: DeleteProfile expected 200, got {} / {:?}",
            deleted.status, deleted.body
        );
        for (operation, reply) in [
            ("GetProfile", em.get_profile(&profile_id).await),
            ("DeleteProfile", em.delete_profile(&profile_id).await),
        ] {
            assert_eq!(
                reply.message(),
                not_found_message(&profile_id),
                "case {operation} after DeleteProfile: expected {:?}, got {:?}",
                not_found_message(&profile_id),
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_associated_profile_names_the_blocking_association() {
    let em = Emulator::new();

    // Rows differing in how many associations block the delete, because the refusal
    // has to name one of them whatever the count is.
    for association_count in 1..=3 {
        let profile_id = em
            .create_named(&format!("blocked-{association_count}"))
            .await
            .nested_string("Profile", "Id");
        let mut association_ids = Vec::new();
        for index in 0..association_count {
            association_ids.push(
                em.associate(
                    &profile_id,
                    &format!("vpc-{association_count}{index}"),
                    &format!("blocking-{association_count}-{index}"),
                )
                .await
                .nested_string("ProfileAssociation", "Id"),
            );
        }
        let case = format!("associations={association_count}");

        let blocked = em.delete_profile(&profile_id).await;
        assert_eq!(
            blocked.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {} / {:?}",
            blocked.status,
            blocked.body
        );
        assert_eq!(
            blocked.status, 400,
            "case {case}: expected HTTP 400, got {}",
            blocked.status
        );
        let message = blocked.message();
        let named = association_ids
            .iter()
            .find(|id| message == still_associated_message(id));
        assert!(
            named.is_some(),
            "case {case}: expected one of {:?} named by the template, got {message:?}",
            association_ids
                .iter()
                .map(|id| still_associated_message(id))
                .collect::<Vec<_>>()
        );
        assert!(
            !message.contains(&profile_id),
            "case {case}: the refusal must name the association, not the Profile \
             {profile_id:?}, got {message:?}"
        );

        // Removing every association clears the refusal: the Profile then deletes.
        for index in 0..association_count {
            em.disassociate(&profile_id, &format!("vpc-{association_count}{index}"))
                .await;
        }
        let deleted = em.delete_profile(&profile_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: with no association left DeleteProfile expected 200, got {} / {:?}",
            deleted.status, deleted.body
        );
    }
}

#[tokio::test]
async fn a_vpc_that_already_has_a_profile_is_refused_as_a_spent_quota() {
    let em = Emulator::new();
    let holder = em
        .create_named("quota-holder")
        .await
        .nested_string("Profile", "Id");
    let rival = em
        .create_named("quota-rival")
        .await
        .nested_string("Profile", "Id");

    // Rows differing in the VPC id's form and in the name the second request gives
    // the association it is refused.
    for (resource_id, rival_name) in [
        ("vpc-12345678", "rival-1"),
        ("vpc-0a1b2c3d4e5f67890", "My_Rival 2"),
        ("v", "r"),
    ] {
        let case = format!("resourceId={resource_id:?} rivalName={rival_name:?}");
        let held = em.associate(&holder, resource_id, "held").await;
        assert_eq!(
            held.status, 200,
            "case {case}: the first association expected 200, got {} / {:?}",
            held.status, held.body
        );

        let refused = em.associate(&rival, resource_id, rival_name).await;
        assert_eq!(
            refused.error.as_deref(),
            Some("LimitExceededException"),
            "case {case}: expected LimitExceededException, got {} / {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {}",
            refused.status
        );
        assert_eq!(
            refused.message(),
            vpc_limit_message(resource_id),
            "case {case}: expected {:?}, got {:?}",
            vpc_limit_message(resource_id),
            refused.message()
        );

        // The same pair again is a different refusal: that association already exists.
        let duplicate = em.associate(&holder, resource_id, "held-again").await;
        assert_eq!(
            duplicate.error.as_deref(),
            Some("ResourceExistsException"),
            "case {case}: a repeat of the same pair must report the association as \
             existing, got {} / {:?}",
            duplicate.status,
            duplicate.body
        );

        // Positive control: once the VPC is released the rival association succeeds,
        // so the refusal is about the spent allowance and nothing else.
        em.disassociate(&holder, resource_id).await;
        let now_allowed = em.associate(&rival, resource_id, rival_name).await;
        assert_eq!(
            now_allowed.status, 200,
            "case {case}: after the VPC was released the association expected 200, \
             got {} / {:?}",
            now_allowed.status,
            now_allowed.body
        );
        em.disassociate(&rival, resource_id).await;
    }
}
