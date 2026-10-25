// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/agent_context_create_limits.rs"]
mod agent_context_create_limits;

#[path = "all/agent_contexts.rs"]
mod agent_contexts;

#[path = "all/agent_goal_listing.rs"]
mod agent_goal_listing;

#[path = "all/agent_goals.rs"]
mod agent_goals;

// Out of alphabetical order ON PURPOSE: declared next to the goal suites it extends
// rather than ahead of them, because the sorted position for this name falls in the one
// gap of this list that every new agent suite lands in, and an insertion there is what
// repeatedly refused to merge.
#[path = "all/agent_goal_create_limits.rs"]
mod agent_goal_create_limits;

// Out of alphabetical order for the reason the note above gives: declared after the
// create-limits suite rather than in the gap every new agent suite sorts into.
#[path = "all/agent_goal_updates.rs"]
mod agent_goal_updates;

#[path = "all/agent_profile_lifecycle.rs"]
mod agent_profile_lifecycle;

#[path = "all/agent_profile_listing.rs"]
mod agent_profile_listing;

#[path = "all/agent_recommendation_generations.rs"]
mod agent_recommendation_generations;

#[path = "all/answer_updates.rs"]
mod answer_updates;

#[path = "all/check_details.rs"]
mod check_details;

#[path = "all/custom_lens_answer_writes.rs"]
mod custom_lens_answer_writes;

#[path = "all/custom_lens_import.rs"]
mod custom_lens_import;

#[path = "all/custom_lens_review_answers.rs"]
mod custom_lens_review_answers;

#[path = "all/custom_lens_versions.rs"]
mod custom_lens_versions;

#[path = "all/delete_idempotency.rs"]
mod delete_idempotency;

#[path = "all/get_agent_context.rs"]
mod get_agent_context;

#[path = "all/get_agent_goal.rs"]
mod get_agent_goal;

#[path = "all/get_agent_profile.rs"]
mod get_agent_profile;

#[path = "all/get_agent_recommendation.rs"]
mod get_agent_recommendation;

#[path = "all/get_answer_reads.rs"]
mod get_answer_reads;

#[path = "all/get_lens.rs"]
mod get_lens;

#[path = "all/lens_listing.rs"]
mod lens_listing;

#[path = "all/lens_resolution.rs"]
mod lens_resolution;

#[path = "all/lens_review_answers.rs"]
mod lens_review_answers;

#[path = "all/lens_review_improvements.rs"]
mod lens_review_improvements;

#[path = "all/lens_review_listing.rs"]
mod lens_review_listing;

#[path = "all/lens_review_notes.rs"]
mod lens_review_notes;

#[path = "all/lens_review_rollup.rs"]
mod lens_review_rollup;

#[path = "all/lens_share_deletion.rs"]
mod lens_share_deletion;

#[path = "all/lens_shares.rs"]
mod lens_shares;

#[path = "all/lens_version_difference.rs"]
mod lens_version_difference;

#[path = "all/lookup_error_envelopes.rs"]
mod lookup_error_envelopes;

#[path = "all/milestone_listing.rs"]
mod milestone_listing;

#[path = "all/milestone_name_uniqueness.rs"]
mod milestone_name_uniqueness;

#[path = "all/no_op_updates.rs"]
mod no_op_updates;

#[path = "all/official_lens_catalog.rs"]
mod official_lens_catalog;

#[path = "all/profile_deletion.rs"]
mod profile_deletion;

#[path = "all/profile_lifecycle.rs"]
mod profile_lifecycle;

#[path = "all/profile_listing.rs"]
mod profile_listing;

#[path = "all/profile_reads.rs"]
mod profile_reads;

#[path = "all/profile_shares.rs"]
mod profile_shares;

#[path = "all/question_choice_catalog.rs"]
mod question_choice_catalog;

#[path = "all/resource_tagging.rs"]
mod resource_tagging;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/review_template_answer_updates.rs"]
mod review_template_answer_updates;

#[path = "all/review_template_answers.rs"]
mod review_template_answers;

#[path = "all/review_template_deletion.rs"]
mod review_template_deletion;

#[path = "all/review_template_lens_review.rs"]
mod review_template_lens_review;

#[path = "all/review_template_lens_review_reads.rs"]
mod review_template_lens_review_reads;

#[path = "all/review_template_lens_review_upgrade.rs"]
mod review_template_lens_review_upgrade;

#[path = "all/review_template_listing.rs"]
mod review_template_listing;

#[path = "all/review_template_lifecycle.rs"]
mod review_template_lifecycle;

#[path = "all/share_invitation_answers.rs"]
mod share_invitation_answers;

#[path = "all/share_invitations.rs"]
mod share_invitations;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/token_ledgers.rs"]
mod token_ledgers;

#[path = "all/untag_resource.rs"]
mod untag_resource;

#[path = "all/validation_envelopes.rs"]
mod validation_envelopes;

#[path = "all/workload_computed_members.rs"]
mod workload_computed_members;

#[path = "all/workload_harness.rs"]
mod workload_harness;

#[path = "all/workload_lens_association.rs"]
mod workload_lens_association;

#[path = "all/workload_lens_disassociation.rs"]
mod workload_lens_disassociation;

#[path = "all/workload_lifecycle.rs"]
mod workload_lifecycle;

#[path = "all/workload_profile_association.rs"]
mod workload_profile_association;

#[path = "all/workload_profile_disassociation.rs"]
mod workload_profile_disassociation;

#[path = "all/workload_profile_upgrade.rs"]
mod workload_profile_upgrade;

#[path = "all/workload_region_invariant.rs"]
mod workload_region_invariant;

#[path = "all/workload_share_permissions.rs"]
mod workload_share_permissions;

#[path = "all/workload_shares.rs"]
mod workload_shares;

// Out of alphabetical order ON PURPOSE, for the reason the `agent_goal_create_limits`
// declaration above gives: the sorted position for an agent-profile suite falls in the one
// gap of this list that every new agent suite lands in, and an insertion there is what
// repeatedly refused to merge. Declared last instead, where nothing else is inserting.
#[path = "all/agent_profile_updates.rs"]
mod agent_profile_updates;

// Out of alphabetical order for the same reason as the declaration above: the sorted
// position for an agent suite is the one gap of this list every new agent suite lands in.
#[path = "all/agent_recommendations.rs"]
mod agent_recommendations;

// Declared last rather than among the `profile_*` suites, for the reason the notes above
// give: an insertion into a crowded run of this list is what repeatedly refused to merge.
#[path = "all/profile_share_deletion.rs"]
mod profile_share_deletion;

// Declared last for the same reason as the declarations above.
#[path = "all/agent_recommendation_paging.rs"]
mod agent_recommendation_paging;

// Declared last for the same reason as the declarations above: an insertion into this
// list's sorted middle is what repeatedly refused to merge.
#[path = "all/global_settings.rs"]
mod global_settings;

// Declared last for the same reason as the declarations above.
#[path = "all/agent_context_updates.rs"]
mod agent_context_updates;
