// SPDX-License-Identifier: CC0-1.0

//! RULE: a resource-based policy belongs to the ONE resource its `ResourceArn` names.
//!
//! CloudTrail keys a stored policy by the addressed resource's own canonical ARN — and
//! an ARN names the resource TYPE, the Region and the identifier — so resources that
//! share an identifier across resource types and Regions carry INDEPENDENT policies, a
//! put replaces the policy of the one resource it addresses and no other, and a
//! resource nothing was attached to keeps answering `ResourcePolicyNotFoundException`
//! however many policies its neighbours carry.
//!
//! This is the one thing a single-resource case cannot show: that the registry is keyed
//! by the resource rather than by the identifier inside its ARN. The table, the seeding
//! and the wire helpers are [`super::resource_policy`]'s, which states what each row
//! varies.

use super::emulator::Emulator;
use super::resource_policy::{
    addressed_resources, attach_each, channel, document, expect_policy, get, put, OTHER_REGION,
};

/// RULE: each addressed resource carries its OWN policy, keyed by its canonical ARN —
/// so an identifier shared across resource types and Regions never shares a policy, and
/// a resource nothing was attached to still reports none.
#[tokio::test]
async fn one_identifier_across_types_and_regions_carries_independent_policies() {
    let emulator = Emulator::new();
    let resources = addressed_resources(&emulator).await;
    // A resource of the table's own kinds that is never attached to, so "carries no
    // policy" is asserted beside five resources that do carry one.
    let unattached = channel(&emulator, "unattached-channel", OTHER_REGION);

    // The ARNs are what the policies are keyed by, so two rows sharing one would make
    // the rest of this case vacuous.
    for (index, resource) in resources.iter().enumerate() {
        for other in resources.iter().skip(index + 1) {
            assert_ne!(
                resource.arn, other.arn,
                "case {:?} vs {:?}: the table's resources must have distinct ARNs, both \
                 are {}",
                resource.case, other.case, resource.arn
            );
        }
    }

    attach_each(&emulator, &resources).await;

    for resource in &resources {
        expect_policy(
            &format!("read back {}", resource.case),
            &get(&emulator, &resource.arn).await,
            &resource.arn,
            &resource.policy,
        );
    }

    get(&emulator, &unattached).await.expect_error(
        "a resource nothing was attached to",
        404,
        "ResourcePolicyNotFoundException",
    );
}

/// RULE: a put replaces the policy of the ONE resource it addresses and leaves every
/// other resource's policy exactly as it was.
///
/// Each row in turn is the resource that gets a second, different document, so the rule
/// is asserted for every resource type and Region in the table rather than for one.
#[tokio::test]
async fn replacing_one_resources_policy_leaves_its_neighbours_alone() {
    let emulator = Emulator::new();
    let resources = addressed_resources(&emulator).await;
    attach_each(&emulator, &resources).await;

    // The document each resource carries right now: a replacement changes exactly one
    // entry, which is what every row is then read against.
    let mut attached: Vec<String> = resources
        .iter()
        .map(|resource| resource.policy.clone())
        .collect();

    for (index, replaced) in resources.iter().enumerate() {
        let replacement = document(&format!("replaced-{}", replaced.case.replace(' ', "-")));
        expect_policy(
            &format!("replace the policy of {}", replaced.case),
            &put(&emulator, &replaced.arn, &replacement).await,
            &replaced.arn,
            &replacement,
        );
        attached[index] = replacement;

        for (position, resource) in resources.iter().enumerate() {
            expect_policy(
                &format!(
                    "{} after replacing the policy of {}",
                    resource.case, replaced.case
                ),
                &get(&emulator, &resource.arn).await,
                &resource.arn,
                &attached[position],
            );
        }
    }
}
