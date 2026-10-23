// SPDX-License-Identifier: CC0-1.0

//! Which IoT COMMANDS exist: the index `StartCommandExecution` validates a
//! `commandArn` against.
//!
//! A command is a CONTROL-PLANE resource — `iot:CreateCommand` creates one,
//! `iot:DeleteCommand` removes it — while the operation that starts an execution
//! of one, `iot-jobs-data:StartCommandExecution`, is served by the IoT Jobs DATA
//! plane, a different crate. Starting an execution is only possible for a command
//! that exists, so the two services have to consult ONE store rather than each
//! keeping a copy of the answer. This module is that store, and it lives in the
//! shared crate because the engine keys state BY TYPE: a type two service crates
//! share must be declared in a crate they both depend on.
//!
//! It holds ARNs and nothing else: the ARN is a command's identity on the wire
//! (`CommandArn`, `arn:<partition>:iot:<region>:<account>:command/<commandId>`),
//! and "does this command exist?" is the whole of what a reader outside the
//! control plane asks. The command's own record — payload, parameters,
//! description — stays with its owner, so this is an index and not a second copy
//! of the resource.
//!
//! WHO WRITES IT: the owner. `iot:CreateCommand` registers the ARN it mints here
//! and `iot:DeleteCommand` removes it. The create is implemented, so a command
//! created through the control plane is in this index from that moment;
//! `iot:DeleteCommand` is still a scaffold answering HTTP 501 (its own issue), so a
//! command's ARN stays here until it is. A `commandArn` this index does not hold
//! names a command that does not exist — which is exactly what
//! `StartCommandExecution` must then refuse. The IoT control plane already
//! carries the same shape of store for the same reason: its `thing_types` /
//! `billing_groups` sets exist so `CreateThing` can refuse a name that is not in
//! them, and stay empty until `CreateThingType` / `CreateBillingGroup` are
//! implemented.
//!
//! Only the TYPE lives here. Each service reaches it through its own
//! `state::command_registry`, beside the accessor for its own store, so this
//! crate needs no dependency on the engine.

use std::collections::BTreeSet;

/// How many commands the index holds at once.
///
/// The set grows only through the control plane's create, so it is
/// client-influenced and has to be bounded; the bound is the IoT Device
/// Management default quota's order of magnitude rather than a published number,
/// and a create past it is refused with [`RegisterRefusal::QuotaExceeded`]
/// instead of growing the store without limit.
pub const MAX_REGISTERED_COMMANDS: usize = 10_000;

/// Why registering a command was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRefusal {
    /// A command with this ARN is already registered: the caller is creating a
    /// command that exists, which its service reports as a conflict.
    AlreadyRegistered,
    /// The index already holds [`MAX_REGISTERED_COMMANDS`] commands.
    QuotaExceeded,
}

impl std::fmt::Display for RegisterRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRegistered => formatter.write_str("the command already exists"),
            Self::QuotaExceeded => write!(
                formatter,
                "the maximum of {MAX_REGISTERED_COMMANDS} commands is already reached"
            ),
        }
    }
}

/// The ARNs of the commands that exist.
///
/// `BTreeSet` (not `HashSet`) so the set has one deterministic order: an index
/// whose content is printed in a log or walked by a future lister must not
/// reorder itself between two runs holding the same commands.
#[derive(Debug, Default)]
pub struct IotCommandRegistry {
    arns: BTreeSet<String>,
}

impl IotCommandRegistry {
    /// Whether a command with this ARN exists.
    ///
    /// Comparison is exact: an ARN carries the partition, region and account the
    /// command was created in, so a command created in one region is not the
    /// command another region's ARN names.
    #[must_use]
    pub fn contains(&self, command_arn: &str) -> bool {
        self.arns.contains(command_arn)
    }

    /// Record that the command `command_arn` names exists, within the index's
    /// bound; `Err` says why a caller's create cannot be recorded.
    pub fn register(&mut self, command_arn: &str) -> Result<(), RegisterRefusal> {
        if self.arns.contains(command_arn) {
            return Err(RegisterRefusal::AlreadyRegistered);
        }
        if self.arns.len() >= MAX_REGISTERED_COMMANDS {
            return Err(RegisterRefusal::QuotaExceeded);
        }
        self.arns.insert(command_arn.to_string());
        Ok(())
    }

    /// Forget the command `command_arn` names, reporting whether it existed.
    pub fn remove(&mut self, command_arn: &str) -> bool {
        self.arns.remove(command_arn)
    }

    /// How many commands exist.
    #[must_use]
    pub fn len(&self) -> usize {
        self.arns.len()
    }

    /// Whether no command exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh index knows no command, whatever ARN is asked about; one
    /// registered ARN is known and its neighbours are not.
    #[test]
    fn only_a_registered_arn_exists() {
        let mut registry = IotCommandRegistry::default();

        // (case, arn, exists once the FIRST row's ARN is registered)
        let rows: [(&str, &str, bool); 4] = [
            (
                "the registered command",
                "arn:aws:iot:us-east-1:111122223333:command/reboot",
                true,
            ),
            (
                "another command id",
                "arn:aws:iot:us-east-1:111122223333:command/restart",
                false,
            ),
            (
                "the same id in another region",
                "arn:aws:iot:eu-west-1:111122223333:command/reboot",
                false,
            ),
            (
                "the same id in another partition",
                "arn:aws-cn:iot:cn-north-1:111122223333:command/reboot",
                false,
            ),
        ];

        for (case, arn, _) in rows {
            assert!(
                !registry.contains(arn),
                "case {case:?}: an empty index must know no command, but it knows {arn:?}"
            );
        }

        registry
            .register(rows[0].1)
            .unwrap_or_else(|refusal| panic!("registering the first command: {refusal}"));

        for (case, arn, expected) in rows {
            let actual = registry.contains(arn);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected contains({arn:?}) == {expected}, got {actual}"
            );
        }
    }

    /// Registering is idempotent-refusing (a second create of one command is a
    /// conflict), removal takes the command back out, and the index is bounded.
    #[test]
    fn registering_is_bounded_and_reversible() {
        let mut registry = IotCommandRegistry::default();
        let arn = "arn:aws:iot:us-east-1:111122223333:command/reboot";

        assert_eq!(registry.register(arn), Ok(()), "the first register succeeds");
        assert_eq!(
            registry.register(arn),
            Err(RegisterRefusal::AlreadyRegistered),
            "case {arn:?}: a second register of one command is a conflict"
        );
        assert!(registry.remove(arn), "case {arn:?}: removing it reports it existed");
        assert!(
            !registry.contains(arn) && registry.is_empty(),
            "case {arn:?}: a removed command no longer exists, got len {}",
            registry.len()
        );
        assert!(
            !registry.remove(arn),
            "case {arn:?}: removing it twice reports it was already gone"
        );

        for index in 0..MAX_REGISTERED_COMMANDS {
            let arn = format!("arn:aws:iot:us-east-1:111122223333:command/bulk-{index}");
            assert_eq!(
                registry.register(&arn),
                Ok(()),
                "case {arn:?}: registering command {index} of the bound must succeed"
            );
        }
        let over = "arn:aws:iot:us-east-1:111122223333:command/over-the-bound";
        assert_eq!(
            registry.register(over),
            Err(RegisterRefusal::QuotaExceeded),
            "case {over:?}: the command past the bound is refused, got len {}",
            registry.len()
        );
        assert_eq!(
            registry.len(),
            MAX_REGISTERED_COMMANDS,
            "a refused register stores nothing"
        );
    }
}
