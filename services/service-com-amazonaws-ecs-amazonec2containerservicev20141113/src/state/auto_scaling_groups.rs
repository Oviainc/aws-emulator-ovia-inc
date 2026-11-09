// SPDX-License-Identifier: CC0-1.0

//! The Auto Scaling group a capacity provider BINDS onto, resolved against Amazon EC2
//! Auto Scaling's own records.
//!
//! `AutoScalingGroupProvider$autoScalingGroupArn` is "The Amazon Resource Name (ARN) that
//! identifies the Auto Scaling group, or the Auto Scaling group name" — a reference to a
//! group of the CALLER's, which Amazon EC2 Auto Scaling holds and this service only points
//! at. A provider is the binding onto the capacity that group runs, and managed scaling
//! drives that group's desired capacity, so a reference naming a group the account does not
//! hold binds onto no capacity at all and is refused.
//!
//! The groups of the account are the Auto Scaling service's own records, so they are read
//! out of that service's store through the one shared engine rather than copied here — the
//! same cross-service read the Amazon VPC networking of a placement is resolved through
//! ([`unresolved_subnet_refusal`]).

use super::*;

/// `ClientException$message` for an `autoScalingGroupProvider` whose reference names no
/// Auto Scaling group of the account.
///
/// A fixed sentence: it states what was not found and what to send instead, and echoes
/// neither the reference the request carried nor the groups that do exist.
pub const AUTO_SCALING_GROUP_NOT_FOUND_MESSAGE: &str =
    "The specified Auto Scaling group does not exist. Specify a valid Auto Scaling group \
     and try again.";

/// The refusal a capacity provider earns when its `autoScalingGroupProvider` names a group
/// Amazon EC2 Auto Scaling holds none of in `region`, or `None` when the reference resolves
/// (and for a provider that names no group at all, which has none to resolve).
///
/// The group is resolved by NAME ([`auto_scaling_group_key`]), because naming one group by
/// ARN and by its bare name names the same group — the same reading the reuse rule compares
/// two providers on. The lookup is made in the Region the request was signed for: a group's
/// instances run in the Region it was created in, so a reference that names a group of
/// another Region names none of this one.
///
/// LOCK ORDER: this takes the Auto Scaling store's lock, holds no other while it does, and
/// hands back a `'static` sentence — so it is called with NO guard of this service's store
/// held, which is what keeps a lock cycle between the two stores unreachable. The same
/// contract [`unresolved_subnet_refusal`] has.
///
/// `None` also covers an engine carrying no Auto Scaling store at all: a refusal has to be
/// grounded in what the account's Auto Scaling state answered, never assumed — the same
/// reading every cross-service resolution in this crate is written to.
pub fn unresolved_auto_scaling_group_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    provider: Option<&AutoScalingGroupProvider>,
) -> Option<&'static str> {
    let reference = provider.map(|provider| provider.auto_scaling_group_arn.clone())?;
    let name = auto_scaling_group_key(&reference);
    let auto_scaling =
        ::service_com_amazonaws_autoscaling_autoscaling_2011_01_01::state::service_state(engine)?;
    let holds_group = {
        let store = auto_scaling.lock();
        store
            .groups
            .values()
            .any(|group| group.name == name && group.region == region)
    };
    (!holds_group).then_some(AUTO_SCALING_GROUP_NOT_FOUND_MESSAGE)
}
