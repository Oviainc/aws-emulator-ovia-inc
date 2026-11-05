// SPDX-License-Identifier: CC0-1.0

//! The TASK SIZES Amazon ECS validates a revision — and anything that overrides one —
//! against.
//!
//! A task's `cpu` and `memory` are free-form strings in the model, so every rule about
//! them is the service's: how a value is READ as the number of units it stands for
//! ([`cpu_units`], [`memory_mib`]), which pairs of them Fargate has a configuration for
//! ([`FARGATE_TASK_SIZES`]), the floor Windows containers on Fargate add to that catalog
//! ([`MIN_WINDOWS_FARGATE_CPU_UNITS`]), and the range the ephemeral storage a task asks
//! for admits ([`EPHEMERAL_STORAGE_SIZE_RANGE_GIB`]).
//!
//! Spelled ONCE here for every operation that carries a task size: the registration that
//! stores one, and the placement that overrides one.

use super::*;

/// How many CPU units one vCPU is, so a `cpu` given "as a string using vCPUs (for
/// example, `1 vCPU` or `1 vcpu`)" is read as the CPU units it stands for
/// (`RegisterTaskDefinition$cpu`).
const CPU_UNITS_PER_VCPU: f64 = 1024.0;

/// How many MiB one GB is, so a `memory` given "as a string using GB (for example,
/// `1GB` or `1 GB`)" is read as the MiB it stands for
/// (`RegisterTaskDefinition$memory`).
const MIB_PER_GB: i64 = 1024;

/// The unit a vCPU-spelled `cpu` names, lower-cased: the member documents `1 vCPU` and
/// `1 vcpu` as the same value, so the spelling is compared case-insensitively.
const VCPU_UNIT: &str = "vcpu";

/// The unit a GB-spelled `memory` names, lower-cased (`1GB` / `1 GB`).
const GB_UNIT: &str = "gb";

/// The largest number of units a `cpu` or `memory` value can stand for and still name a
/// size: ten times the largest the documented catalog states (16384 CPU units, 122880
/// MiB), so every size a task could ask for converts while a value no task size reaches
/// does not become a saturated count.
const MAX_STATED_UNITS: f64 = 1_228_800.0;

/// The task memory a Fargate task size allows, in MiB.
enum FargateMemory {
    /// Exactly the values the doc lists for this CPU size — they are not one
    /// progression: 256 CPU units allow 512, 1024 and 2048 MiB, skipping 1536.
    Listed(&'static [i64]),
    /// Every multiple of `step` MiB from `min` through `max`, the form the doc states as
    /// "Between X and Y in increments of Z".
    Stepped { min: i64, max: i64, step: i64 },
}

/// One Fargate task size: the task CPU units, and the task memory those units support.
///
/// The catalog below is the `RegisterTaskDefinition$memory` doc's own list of legal
/// pairs ("If using the Fargate launch type, this field is required and you must use one
/// of the following values. This determines your range of supported values for the `cpu`
/// parameter"), read as CPU units -> memory. A pair outside it is a combination Fargate
/// has no configuration for.
struct FargateTaskSize {
    cpu: i64,
    memory: FargateMemory,
}

/// The Fargate task sizes, from the `memory` member's documented list.
const FARGATE_TASK_SIZES: [FargateTaskSize; 7] = [
    // "512 (0.5 GB), 1024 (1 GB), 2048 (2 GB) - Available cpu values: 256 (.25 vCPU)"
    FargateTaskSize {
        cpu: 256,
        memory: FargateMemory::Listed(&[512, 1024, 2048]),
    },
    // "1024 (1 GB), 2048 (2 GB), 3072 (3 GB), 4096 (4 GB) - Available cpu values: 512"
    FargateTaskSize {
        cpu: 512,
        memory: FargateMemory::Stepped {
            min: 1024,
            max: 4096,
            step: 1024,
        },
    },
    // "2048 (2 GB) ... 8192 (8 GB) - Available cpu values: 1024 (1 vCPU)"
    FargateTaskSize {
        cpu: 1024,
        memory: FargateMemory::Stepped {
            min: 2048,
            max: 8192,
            step: 1024,
        },
    },
    // "Between 4096 (4 GB) and 16384 (16 GB) in increments of 1024 (1 GB) - 2048 vCPU"
    FargateTaskSize {
        cpu: 2048,
        memory: FargateMemory::Stepped {
            min: 4096,
            max: 16384,
            step: 1024,
        },
    },
    // "Between 8192 (8 GB) and 30720 (30 GB) in increments of 1024 (1 GB) - 4096 vCPU"
    FargateTaskSize {
        cpu: 4096,
        memory: FargateMemory::Stepped {
            min: 8192,
            max: 30720,
            step: 1024,
        },
    },
    // "Between 16 GB and 60 GB in 4 GB increments - Available cpu values: 8192 (8 vCPU)"
    FargateTaskSize {
        cpu: 8192,
        memory: FargateMemory::Stepped {
            min: 16 * MIB_PER_GB,
            max: 60 * MIB_PER_GB,
            step: 4 * MIB_PER_GB,
        },
    },
    // "Between 32GB and 120 GB in 8 GB increments - Available cpu values: 16384 (16 vCPU)"
    FargateTaskSize {
        cpu: 16384,
        memory: FargateMemory::Stepped {
            min: 32 * MIB_PER_GB,
            max: 120 * MIB_PER_GB,
            step: 8 * MIB_PER_GB,
        },
    },
];

/// The CPU units a `cpu` value stands for, or `None` when it spells no number of them.
///
/// "It can be expressed as an integer using CPU units (for example, `1024`) or as a
/// string using vCPUs (for example, `1 vCPU` or `1 vcpu`) in a task definition." A value
/// that is negative, or too large or too small to be a number of vCPUs, names no task
/// size either, so it carries no units rather than a saturated count.
pub fn cpu_units(cpu: &str) -> Option<i64> {
    let cpu = cpu.trim().to_ascii_lowercase();
    match cpu.strip_suffix(VCPU_UNIT) {
        Some(count) => {
            let vcpus: f64 = count.trim().parse().ok()?;
            units_of(vcpus * CPU_UNITS_PER_VCPU)
        }
        None => cpu.parse().ok().filter(|units| *units >= 0),
    }
}

/// The MiB a `memory` value stands for, or `None` when it spells no amount of it.
///
/// "It can be expressed as an integer using MiB (for example ,`1024`) or as a string
/// using GB (for example, `1GB` or `1 GB`) in a task definition." As with `cpu`, a value
/// outside the amounts a task can ask for carries no MiB at all.
pub fn memory_mib(memory: &str) -> Option<i64> {
    let memory = memory.trim().to_ascii_lowercase();
    match memory.strip_suffix(GB_UNIT) {
        Some(count) => {
            let gb: f64 = count.trim().parse().ok()?;
            units_of(gb * MIB_PER_GB as f64)
        }
        None => memory.parse().ok().filter(|mib| *mib >= 0),
    }
}

/// The whole number of units `scaled` stands for, or `None` when it is not a number a
/// task size could carry: a non-finite value, a negative one, or one past what the
/// largest Fargate size is stated in.
fn units_of(scaled: f64) -> Option<i64> {
    if !scaled.is_finite() || !(0.0..=MAX_STATED_UNITS).contains(&scaled) {
        return None;
    }
    Some(scaled.round() as i64)
}

/// Whether Fargate has a task size for this pair of CPU units and MiB
/// ([`FARGATE_TASK_SIZES`]).
pub fn is_fargate_task_size(cpu: i64, memory: i64) -> bool {
    FARGATE_TASK_SIZES
        .iter()
        .filter(|size| size.cpu == cpu)
        .any(|size| match size.memory {
            FargateMemory::Listed(values) => values.contains(&memory),
            FargateMemory::Stepped { min, max, step } => {
                memory >= min && memory <= max && (memory - min) % step == 0
            }
        })
}

/// A `cpu` or `memory` value as a refusal states it: the number of units it stands for,
/// or the text itself when it stands for none.
pub fn stated_units(value: &str, units: Option<i64>) -> String {
    match units {
        Some(units) => units.to_string(),
        None => value.trim().to_string(),
    }
}

/// `ClientException$message` for a task size Fargate has no configuration for: the pair
/// is stated as the units it stands for ([`stated_units`]) and the caller is pointed at
/// the published catalog, so nothing of the request but the two values the caller has to
/// change reaches the body.
///
/// Spelled once for every registration that stores a task size, so one pair cannot be
/// refused by one and accepted by another.
pub fn fargate_task_size_message(cpu: &str, memory: &str) -> String {
    format!(
        "No Fargate configuration exists for given values: {} CPU, {} memory. See the \
         Amazon ECS documentation for the valid values.",
        stated_units(cpu, cpu_units(cpu)),
        stated_units(memory, memory_mib(memory))
    )
}

/// The same refusal where the task size came from a PLACEMENT's `overrides` rather than
/// from the registration: the one fixed sentence, naming neither value.
pub const FARGATE_TASK_SIZE_MESSAGE: &str = "No Fargate configuration exists for given values.";

/// Whether Fargate has a configuration for a task of this `cpu`/`memory` running
/// `os_family`: the pair has to be one of the published task sizes
/// ([`is_fargate_task_size`]), and a Windows task has to meet the Windows floor
/// ([`MIN_WINDOWS_FARGATE_CPU_UNITS`]) on top of that. A value that spells no number of
/// units names no configuration at all.
pub fn is_fargate_configuration(cpu: &str, memory: &str, os_family: &str) -> bool {
    let (Some(units), Some(mib)) = (cpu_units(cpu), memory_mib(memory)) else {
        return false;
    };
    if is_windows_os_family(os_family) && units < MIN_WINDOWS_FARGATE_CPU_UNITS {
        return false;
    }
    is_fargate_task_size(units, mib)
}

/// The refusal a REGISTERED task size earns when Fargate has no configuration for it on
/// the operating system the revision declares, or `None` when it names one.
pub fn fargate_task_size_violation(cpu: &str, memory: &str, os_family: &str) -> Option<String> {
    (!is_fargate_configuration(cpu, memory, os_family))
        .then(|| fargate_task_size_message(cpu, memory))
}

/// The smallest task `cpu`, in CPU units, a WINDOWS container on Fargate runs at.
///
/// The Fargate task sizes above are the whole catalog for Linux; Windows containers on
/// Fargate are published with a higher floor — the smallest supported Windows task size
/// is 1 vCPU (the ECS developer guide's "Windows containers on AWS Fargate" supported
/// configurations start at 1 vCPU with 2 GB of memory), so the quarter- and half-vCPU
/// sizes a Linux task may ask for name no Windows configuration at all.
pub const MIN_WINDOWS_FARGATE_CPU_UNITS: i64 = 1024;

/// The `OSFamily` prefix every WINDOWS operating-system family's name carries, upper-cased
/// for the comparison below. The enum's Windows values are each named for a Windows Server
/// release (`WINDOWS_SERVER_2019_CORE` and the rest), so the prefix is what they share —
/// including a release added to the enum later.
const WINDOWS_OS_FAMILY_PREFIX: &str = "WINDOWS";

/// Whether `os_family` is one of the WINDOWS operating-system families.
///
/// The `OSFamily` enum (`smithy_query(query="shape", shape="OSFamily")`) spans `LINUX` and
/// the Windows Server families, so a Windows family is one whose name carries
/// [`WINDOWS_OS_FAMILY_PREFIX`]. Read off the name rather than as "not Linux", so a value
/// the enum does not carry at all earns neither the Windows floor below nor the Windows
/// platform-version pairing.
pub fn is_windows_os_family(os_family: &str) -> bool {
    os_family
        .trim()
        .to_ascii_uppercase()
        .starts_with(WINDOWS_OS_FAMILY_PREFIX)
}

/// The operating-system family a stored or requested `runtimePlatform` names: the one it
/// declares, else the Linux default a revision that declares none runs on
/// ([`DEFAULT_OS_FAMILY`]).
pub fn requested_os_family(runtime_platform: Option<&RuntimePlatform>) -> String {
    runtime_platform
        .and_then(|platform| platform.operating_system_family.clone())
        .unwrap_or_else(|| DEFAULT_OS_FAMILY.to_string())
}

/// The inclusive range, in GiB, `EphemeralStorage$sizeInGiB` admits: "The minimum
/// supported value is 21 GiB and the maximum supported value is 200 GiB" — the member's
/// own doc.
pub const EPHEMERAL_STORAGE_SIZE_RANGE_GIB: (i32, i32) = (21, 200);

/// `ClientException$message`/`InvalidParameterException$message` for an
/// `ephemeralStorage.sizeInGiB` outside the range the member admits. Amazon ECS names the
/// structure and the ONE bound the value crossed, and echoes neither the value the
/// request carried nor the other bound.
pub fn ephemeral_storage_size_message(bound: &str, limit: i32) -> String {
    format!("EphemeralStorage size should be at {bound} {limit}")
}

/// The refusal an `ephemeralStorage` a request carries earns when the size it asks for is
/// outside [`EPHEMERAL_STORAGE_SIZE_RANGE_GIB`], or `None` when it asks for a size in
/// range (or asks for none at all).
///
/// Spelled once for every operation that carries the structure — the registration that
/// stores one, and the placement that overrides one — so a size one of them refuses
/// cannot be written by another.
pub fn ephemeral_storage_violation(storage: Option<&EphemeralStorage>) -> Option<String> {
    let storage = storage?;
    let (min, max) = EPHEMERAL_STORAGE_SIZE_RANGE_GIB;
    if storage.size_in_gi_b < min {
        return Some(ephemeral_storage_size_message("least", min));
    }
    if storage.size_in_gi_b > max {
        return Some(ephemeral_storage_size_message("most", max));
    }
    None
}
