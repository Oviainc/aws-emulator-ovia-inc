// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice service.
//!
//! EC2 Instance Connect pushes a short-lived SSH public key at an EC2 instance, so every
//! answer either operation gives is computed from four things kept here:
//!
//! * [`ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState::instances`] — the
//!   FALLBACK instance registry: whether an instance exists, which Availability Zone it is
//!   in, what lifecycle state it is in, and whether its instance type supports the serial
//!   console. An instance is EC2's resource, not this service's (this service models no
//!   operation that launches one), so a push ASKS EC2 about the id it names ([`ec2`]) and
//!   what EC2 answers decides it; this registry is what a push reads only when no EC2 can
//!   be asked at all (see [`seed_instances`]).
//! * `pushed_keys` / `serial_console_keys` — the keys that were pushed, each with the
//!   instant it expires. An expired key is treated as absent on lookup.
//! * `serial_console_access_enabled` — the account-level serial-console toggle that
//!   `SerialConsoleAccessDisabledException` reports.
//! * `requests` — a bounded log of the calls served, so a caller can tell a recorded call
//!   from a canned reply.
//!
//! It also owns the one modelled input constraint the framework's boundary validator
//! cannot enforce — `InstanceOSUser`'s `@pattern` — because both the handler that enforces
//! it and `service.rs`, which words the boundary's own refusals, state it the same way.
//!
//! Whether the caller may push at all is [`authorization`]: the IAM decision both operations
//! take before they resolve anything.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aws_smithy_schema_server::constraints::{path, Violation, ViolationKind};

pub mod authorization;
pub mod ec2;

/// The Region a hop into another service is made in when the request carries no SigV4
/// credential scope to read one from (an unsigned call). Matches the Region the emulator's
/// own generated shared config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope.
///
/// Deliberately a per-crate copy of a helper other service crates also hold: the function
/// that owns the parsing (`emulator_server::sigv4_credential_scope`) lives in the vendored,
/// read-only framework.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// How long a pushed key stays usable.
///
/// 60 seconds, from this service's own model: `SendSSHPublicKey` documents "The key remains
/// for 60 seconds." and `SendSerialConsoleSSHPublicKey` "The key remains for 60 seconds,
/// which gives you 60 seconds to establish a serial console connection to the instance
/// using SSH." — a duration the service documents, so it keeps that value.
pub const PUSHED_KEY_TTL: Duration = Duration::from_secs(60);

/// The only serial port the model admits (`SerialPort`, range 0..0, default 0 — "Currently
/// only port 0 is supported").
pub const SERIAL_CONSOLE_PORT: i32 = 0;

/// How many served calls [`ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState::requests`]
/// keeps. The log exists so a test can see that a call was recorded and that request ids do
/// not repeat; it is bounded because a long-lived emulator must not grow a list forever.
pub const MAX_RECORDED_REQUESTS: usize = 256;

// ---------------------------------------------------------------------------
// The one modelled constraint the boundary validator cannot enforce
// ---------------------------------------------------------------------------
//
// The framework validates every constraint the model declares before a handler runs
// (`@length`, `@range`, and any `@pattern` the `regex` crate can compile), and
// `service.rs::override_refusal` words those refusals as this service's own
// `InvalidArgsException`. ONE constraint escapes it: `InstanceOSUser`'s `@pattern`, whose
// second branch opens with a lookahead, which `regex` cannot compile — and the framework
// treats a pattern it cannot compile as matching. That member is therefore validated here,
// and nothing else is: every other member's bounds and patterns stay the model's single
// source of truth in the generated member schemas.

/// `InstanceOSUser`'s `@pattern`, verbatim from the model
/// (`smithy_query(query="operation",operation="SendSSHPublicKey")`).
///
/// Named in the refusal clause; what EVALUATES it is
/// [`instance_os_user_satisfies_pattern`], which implements this expression branch by
/// branch.
pub const INSTANCE_OS_USER_PATTERN: &str = r"^(^[A-Za-z_][A-Za-z0-9\@\._-]{0,30}[A-Za-z0-9\$_-]?$)|(^(?=.{2,32}$)[0-9]{1,32}[A-Za-z\@\._-][A-Za-z0-9\@\._-]*[A-Za-z0-9\$_-]?$)$";

/// The Smithy name of the member [`INSTANCE_OS_USER_PATTERN`] constrains. A clause names it
/// lowerCamel — the spelling [`path::member`] produces — so a clause built here cannot
/// drift from one the framework builds for the same member.
pub const INSTANCE_OS_USER_MEMBER: &str = "InstanceOSUser";

/// `InstanceOSUser`'s `@length` maximum (1..32 in the model). Used only to bound the value
/// a refusal echoes: a longer value is refused for its length by the framework, whose
/// `@length` check compiles perfectly well.
pub const INSTANCE_OS_USER_MAX_LENGTH: usize = 32;

/// `[A-Za-z_]` — what the pattern's first branch may open with.
fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// `[A-Za-z0-9\@\._-]` — the interior class of both branches.
fn is_name_interior(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-')
}

/// `[A-Za-z0-9\$_-]` — the optional final character of both branches.
fn is_name_end(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '$' | '_' | '-')
}

/// `[A-Za-z\@\._-]` — what must follow the second branch's leading digits.
fn is_after_digits(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '@' | '.' | '_' | '-')
}

/// `[A-Za-z0-9\@\._-]{0,max_interior}[A-Za-z0-9\$_-]?` over the whole of `tail`.
///
/// Either every character is an interior one, or the last is a final one and everything
/// before it is interior. One left-to-right scan, no backtracking.
fn tail_satisfies(tail: &[char], max_interior: usize) -> bool {
    let all_interior = tail.len() <= max_interior && tail.iter().copied().all(is_name_interior);
    let interior_then_end = match tail.split_last() {
        Some((last, interior)) => {
            interior.len() <= max_interior
                && interior.iter().copied().all(is_name_interior)
                && is_name_end(*last)
        }
        None => false,
    };
    all_interior || interior_then_end
}

/// Whether `value` satisfies [`INSTANCE_OS_USER_PATTERN`].
///
/// The expression is an alternation of two self-anchored branches, so a Smithy *find* of it
/// is a whole-value match of one of them, and each branch is a fixed sequence of character
/// classes:
///
/// * `^[A-Za-z_][A-Za-z0-9\@\._-]{0,30}[A-Za-z0-9\$_-]?$` — a name opening with a letter or
///   an underscore;
/// * `^(?=.{2,32}$)[0-9]{1,32}[A-Za-z\@\._-][A-Za-z0-9\@\._-]*[A-Za-z0-9\$_-]?$` — a name
///   opening with digits, at least two characters long (which is all the lookahead says),
///   whose first non-digit is a letter or one of `@._-`.
///
/// It is evaluated by scanning those classes rather than by a regex engine. The `regex`
/// crate — the linear-time engine the framework itself matches with — cannot compile the
/// lookahead, and the merge gate's security-requirements review (analysis record MIT-4013,
/// "Keep untrusted-input matching linear-time in per-service handlers") reports a
/// backtracking engine on a handler's untrusted-input path as unmet, naming a hand-written
/// validator as the alternative. The scan is a single left-to-right pass over the value, so
/// its cost is linear in the value's length whatever the value is. The leading-digit count
/// determines the second branch's split point uniquely, because the class that must follow
/// those digits contains no digit.
pub fn instance_os_user_satisfies_pattern(value: &str) -> bool {
    let chars: Vec<char> = value.chars().collect();

    // First branch: a leading name character, then the interior/final tail (at most 30
    // interior characters, as `{0,30}` says).
    if let Some((first, tail)) = chars.split_first() {
        if is_name_start(*first) && tail_satisfies(tail, 30) {
            return true;
        }
    }

    // Second branch: the lookahead is a whole-value length assertion.
    if !(2..=32).contains(&chars.len()) {
        return false;
    }
    let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    match chars.get(digits) {
        // `[A-Za-z0-9\@\._-]*` is unbounded in the second branch, so the tail's interior
        // length is bounded only by the value itself.
        Some(after) if is_after_digits(*after) => tail_satisfies(&chars[digits + 1..], usize::MAX),
        _ => false,
    }
}

/// The `@pattern` violation `value` commits as `InstanceOSUser`, or `None` when it
/// satisfies the pattern.
pub fn instance_os_user_violation(value: &str) -> Option<Violation> {
    if instance_os_user_satisfies_pattern(value) {
        return None;
    }
    Some(Violation {
        path: path::member(INSTANCE_OS_USER_MEMBER),
        kind: ViolationKind::Pattern(INSTANCE_OS_USER_PATTERN.to_string()),
        value: Some(echoed_within_member_length(value)),
        sensitive: false,
    })
}

/// The rejected value as a clause should show it, elided to the member's own modelled
/// maximum.
///
/// A longer value is refused for its `@length` by the framework, so this only ever elides a
/// value that could not have got here; it is bounded anyway because the caller chooses the
/// length of what it sent and a clause interpolates it.
fn echoed_within_member_length(value: &str) -> String {
    let shown: String = value.chars().take(INSTANCE_OS_USER_MAX_LENGTH).collect();
    let length = value.chars().count();
    if length <= INSTANCE_OS_USER_MAX_LENGTH {
        shown
    } else {
        format!("{shown}... [{length} characters]")
    }
}

/// The message an `InvalidArgsException` carries for `violations`, or `None` when there are
/// none.
///
/// Built by the framework's own envelope (`N validation error(s) detected: Value '<value>'
/// at '<member>' failed to satisfy constraint: <requirement>`), so a violation this service
/// catches in a handler is worded exactly like one the boundary validator catches. BOTH
/// paths come through here — the handler's own check and `service.rs`'s refusal renderer —
/// so the two cannot word the same refusal differently.
pub fn invalid_args_message(violations: &[Violation]) -> Option<String> {
    let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
    aws_smithy_schema_server::envelope(&stated)
}

/// A violation as this service states it: the framework's clause, with a `@pattern` named
/// without the anchors that bracket it.
///
/// A clause already says the MEMBER must satisfy the expression, so the anchors that make
/// the model's pattern whole-string would restate the sentence; this service names the
/// expression alone (`i-[a-f0-9]+` for `InstanceId`'s `^i-[a-f0-9]+$`). Anchors INSIDE the
/// expression belong to it and stay. What is ENFORCED is the model's pattern, unchanged —
/// this is the rendering of the clause only. Every other clause is the framework's,
/// unchanged.
fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(regex) => Violation {
            kind: ViolationKind::Pattern(unanchored(regex).to_string()),
            ..violation.clone()
        },
        _ => violation.clone(),
    }
}

/// `regex` without a leading `^` or a trailing `$`. A trailing `$` escaped by a backslash
/// is a literal dollar in the expression, not an anchor, so it stays.
fn unanchored(regex: &str) -> &str {
    let inner = regex.strip_prefix('^').unwrap_or(regex);
    match inner.strip_suffix('$') {
        Some(stripped) if !stripped.ends_with('\\') => stripped,
        _ => inner,
    }
}

/// An instance's lifecycle state, as EC2 names `instance-state-name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceState {
    /// Launching; not yet reachable.
    Pending,
    /// Running — the only state a key can be pushed to.
    Running,
    /// Being stopped.
    Stopping,
    /// Stopped.
    Stopped,
    /// Being terminated.
    ShuttingDown,
    /// Terminated.
    Terminated,
}

/// Why an instance in a given state cannot be connected to.
///
/// Both operations model both outcomes, and each maps this to its own error variant, so the
/// state-to-outcome rule lives in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectBlocked {
    /// The instance exists but has not finished launching: the modelled
    /// `EC2InstanceUnavailableException` ("The instance is currently unavailable. Wait a few
    /// minutes and try again.").
    NotYetAvailable,
    /// The instance is shutting down, stopped or terminated: the modelled
    /// `EC2InstanceStateInvalidException` ("Connecting to a stopped or terminated instance is
    /// not supported.").
    InvalidState,
}

impl InstanceState {
    /// `None` when a key may be pushed to an instance in this state, else why it may not.
    pub fn blocks_connect(self) -> Option<ConnectBlocked> {
        match self {
            Self::Running => None,
            Self::Pending => Some(ConnectBlocked::NotYetAvailable),
            Self::Stopping | Self::Stopped | Self::ShuttingDown | Self::Terminated => {
                Some(ConnectBlocked::InvalidState)
            }
        }
    }

    /// The name EC2 gives this state, for an error message that says what was found.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::ShuttingDown => "shutting-down",
            Self::Terminated => "terminated",
        }
    }

    /// The state EC2 named, read back off the wire: the `InstanceStateName` value
    /// `DescribeInstances` reports in `Instance$State` ([`Self::as_str`] is the same
    /// spelling the other way round). `None` for a value this service does not know, so a
    /// caller never guesses a lifecycle state EC2 did not name.
    pub fn from_ec2_name(name: &str) -> Option<Self> {
        [
            Self::Pending,
            Self::Running,
            Self::Stopping,
            Self::Stopped,
            Self::ShuttingDown,
            Self::Terminated,
        ]
        .into_iter()
        .find(|state| state.as_str() == name)
    }
}

/// A registered EC2 instance: everything this service needs to decide a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// The instance id (`i-...`) the registry is keyed by.
    pub instance_id: String,
    /// The Availability Zone the instance was launched in; a request that names a different
    /// one is refused.
    pub availability_zone: String,
    /// The instance type, reported in the serial-console refusal.
    pub instance_type: String,
    /// The lifecycle state.
    pub state: InstanceState,
    /// Whether this instance type supports the EC2 serial console ("Only Nitro instance
    /// types are currently supported", per `EC2InstanceTypeInvalidException`).
    pub serial_console_supported: bool,
    /// The instance's tags, as EC2 reports them (`Instance$Tags`), keyed by tag key.
    ///
    /// Kept because an authorization decision about the instance is made over them: an
    /// identity policy may condition a key push on the tags of the instance it names
    /// ([`authorization::Conditions`]), and the tag set is the instance's own, so it is read
    /// from whoever answered for the instance rather than invented here. A record whose tags
    /// nothing reported carries none, which is not the same as an instance with no tags —
    /// see [`authorization::Conditions::new`], which only publishes the tag condition keys
    /// for a tag set that IS known.
    pub tags: BTreeMap<String, String>,
}

impl Instance {
    /// A registry entry.
    pub fn new(
        instance_id: impl Into<String>,
        availability_zone: impl Into<String>,
        instance_type: impl Into<String>,
        state: InstanceState,
        serial_console_supported: bool,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            availability_zone: availability_zone.into(),
            instance_type: instance_type.into(),
            state,
            serial_console_supported,
            tags: BTreeMap::new(),
        }
    }

    /// The same entry in another lifecycle state (a stop, a start, a termination).
    pub fn in_state(mut self, state: InstanceState) -> Self {
        self.state = state;
        self
    }

    /// The same entry carrying `tags`, the tags the owner of the instance reports for it.
    pub fn with_tags<K: Into<String>, V: Into<String>>(
        mut self,
        tags: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.tags = tags
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }
}

/// A key that was pushed, and when it stops being usable.
#[derive(Debug, Clone)]
pub struct PushedKey {
    /// The public key material exactly as the caller sent it.
    pub public_key: String,
    /// The OS user the key authenticates, or `None` for a serial-console push — the serial
    /// console takes no OS user, so its keys are keyed by port instead.
    pub os_user: Option<String>,
    /// When the push was served.
    pub pushed_at: Instant,
    /// `pushed_at` plus [`PUSHED_KEY_TTL`]; from this instant the key is gone.
    pub expires_at: Instant,
    /// The id of the request that pushed it.
    pub request_id: String,
}

impl PushedKey {
    fn new(
        public_key: &str,
        os_user: Option<&str>,
        request_id: &str,
        now: Instant,
    ) -> Self {
        Self {
            public_key: public_key.to_string(),
            os_user: os_user.map(str::to_string),
            pushed_at: now,
            expires_at: now + PUSHED_KEY_TTL,
            request_id: request_id.to_string(),
        }
    }

    /// Whether the key's lifetime has run out at `now`.
    pub fn is_expired_at(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    /// How much of the key's lifetime is left at `now` (zero once expired).
    pub fn remaining_at(&self, now: Instant) -> Duration {
        self.expires_at.saturating_duration_since(now)
    }
}

/// One served call: enough to show the call happened and which id it answered with.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    /// The `RequestId` returned to the caller.
    pub request_id: String,
    /// The operation name.
    pub operation: String,
    /// The instance the call named.
    pub instance_id: String,
    /// When it was served.
    pub at: Instant,
}

/// Per-service state for com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState {
    /// The instance registry, keyed by instance id.
    pub instances: HashMap<String, Instance>,
    /// Keys pushed by `SendSSHPublicKey`, keyed by (instance id, OS user).
    pub pushed_keys: HashMap<(String, String), PushedKey>,
    /// Keys pushed by `SendSerialConsoleSSHPublicKey`, keyed by (instance id, serial port).
    pub serial_console_keys: HashMap<(String, i32), PushedKey>,
    /// The account-level EC2 serial console toggle (EC2's `EnableSerialConsoleAccess` /
    /// `DisableSerialConsoleAccess`), which the serial-console refusal reads when EC2
    /// cannot be asked for it ([`ec2::serial_console_access_enabled`]).
    ///
    /// Starts DISABLED, which is the account's own starting value: serial-console access
    /// is granted at the account level and is not granted until `EnableSerialConsoleAccess`
    /// grants it — the modelled `SerialConsoleAccessDisabledException` says so itself ("To
    /// authorize your account, run the EnableSerialConsoleAccess API"). That API belongs to
    /// EC2, not to this service, so where EC2 is mounted its own status is what decides.
    pub serial_console_access_enabled: bool,
    /// The most recent served calls, newest last, at most [`MAX_RECORDED_REQUESTS`].
    pub requests: Vec<RequestRecord>,
    /// A seed drawn once per process, so two runs of the emulator never answer with the same
    /// `RequestId` values.
    request_id_seed: u64,
    /// How many calls this store has answered; never repeats, so no `RequestId` does.
    request_sequence: u64,
}

/// A value that differs per process, from the seed the OS gives std's [`RandomState`].
fn process_seed() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u8(0);
    hasher.finish()
}

/// The instances this service falls back on when EC2 cannot be asked.
///
/// An instance is EC2's resource and a push asks EC2 about the id it names ([`ec2`]), so in
/// an emulator that mounts EC2 these records are not what decides a push. They exist for
/// the emulator that mounts this service ALONE, where no service can answer for an
/// instance and an empty registry would answer 404 for every push: four instances that
/// between them reach every branch the two operations have (running Nitro, stopped, running
/// but not serial-console capable, terminated). A test registers its own instances with
/// [`ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState::register_instance`].
///
/// The zone names use `us-east-1`, the region the emulator writes into the SDK shared config
/// it hands clients (`vendor/server/src/lib.rs`, `region = us-east-1`).
pub fn seed_instances() -> Vec<Instance> {
    vec![
        // Nitro, running: both operations succeed against this one.
        Instance::new(
            "i-0a1b2c3d4e5f60718",
            "us-east-1a",
            "m5.large",
            InstanceState::Running,
            true,
        ),
        // Stopped: `EC2InstanceStateInvalidException`.
        Instance::new(
            "i-0b2c3d4e5f6071829",
            "us-east-1b",
            "m5.large",
            InstanceState::Stopped,
            true,
        ),
        // Running, but a pre-Nitro instance type: the serial console refuses it with
        // `EC2InstanceTypeInvalidException` while `SendSSHPublicKey` still succeeds.
        Instance::new(
            "i-0c3d4e5f607182930",
            "us-east-1c",
            "m3.medium",
            InstanceState::Running,
            false,
        ),
        // Terminated: `EC2InstanceStateInvalidException`.
        Instance::new(
            "i-0d4e5f60718293041",
            "us-east-1a",
            "m5.large",
            InstanceState::Terminated,
            true,
        ),
    ]
}

impl Default for ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState {
    fn default() -> Self {
        let mut state = Self {
            instances: HashMap::new(),
            pushed_keys: HashMap::new(),
            serial_console_keys: HashMap::new(),
            serial_console_access_enabled: false,
            requests: Vec::new(),
            request_id_seed: process_seed(),
            request_sequence: 0,
        };
        for instance in seed_instances() {
            state.register_instance(instance);
        }
        state
    }
}

impl ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState {
    /// A fresh `RequestId` for a call about to be served.
    ///
    /// AWS answers both operations with an opaque UUID-shaped id, one per call. This builds
    /// one by hashing (`sha2`) the per-process seed, the per-call sequence number and the
    /// wall clock, and rendering the first 128 bits as hex (`hex`) in the 8-4-4-4-12 form a
    /// client's UUID parser accepts. The sequence number never repeats, so neither does an
    /// id within a run; the seed differs per process, so neither do ids between runs.
    pub fn next_request_id(&mut self) -> String {
        use sha2::Digest;

        self.request_sequence = self.request_sequence.wrapping_add(1);
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        let digest = sha2::Sha256::digest(format!(
            "{:016x}:{:016x}:{:032x}",
            self.request_id_seed, self.request_sequence, since_epoch
        ));
        let rendered = hex::encode(&digest[..16]);
        format!(
            "{}-{}-{}-{}-{}",
            &rendered[0..8],
            &rendered[8..12],
            &rendered[12..16],
            &rendered[16..20],
            &rendered[20..32]
        )
    }

    /// Add (or replace) a registry entry, returning the entry it displaced.
    pub fn register_instance(&mut self, instance: Instance) -> Option<Instance> {
        self.instances.insert(instance.instance_id.clone(), instance)
    }

    /// The registered instance with this id, if the registry holds one.
    pub fn instance(&self, instance_id: &str) -> Option<&Instance> {
        self.instances.get(instance_id)
    }

    /// Move a registered instance to another lifecycle state; `false` when it is unknown.
    pub fn set_instance_state(&mut self, instance_id: &str, state: InstanceState) -> bool {
        match self.instances.get_mut(instance_id) {
            Some(instance) => {
                instance.state = state;
                true
            }
            None => false,
        }
    }

    /// Store a key against (instance, OS user), replacing any key already there and
    /// restarting its lifetime at `now`. Returns the stored record.
    pub fn push_ssh_key(
        &mut self,
        instance_id: &str,
        os_user: &str,
        public_key: &str,
        request_id: &str,
        now: Instant,
    ) -> PushedKey {
        self.drop_expired_keys(now);
        let pushed = PushedKey::new(public_key, Some(os_user), request_id, now);
        self.pushed_keys.insert(
            (instance_id.to_string(), os_user.to_string()),
            pushed.clone(),
        );
        pushed
    }

    /// Store a serial-console key against (instance, serial port), replacing any key already
    /// there and restarting its lifetime at `now`. Returns the stored record.
    pub fn push_serial_console_key(
        &mut self,
        instance_id: &str,
        serial_port: i32,
        public_key: &str,
        request_id: &str,
        now: Instant,
    ) -> PushedKey {
        self.drop_expired_keys(now);
        let pushed = PushedKey::new(public_key, None, request_id, now);
        self.serial_console_keys
            .insert((instance_id.to_string(), serial_port), pushed.clone());
        pushed
    }

    /// The key usable for (instance, OS user) at `now`. An expired key reads as absent,
    /// whether or not anything has pruned it yet.
    pub fn active_ssh_key(
        &self,
        instance_id: &str,
        os_user: &str,
        now: Instant,
    ) -> Option<&PushedKey> {
        self.pushed_keys
            .get(&(instance_id.to_string(), os_user.to_string()))
            .filter(|key| !key.is_expired_at(now))
    }

    /// The serial-console key usable for (instance, port) at `now`, expiry applied as above.
    pub fn active_serial_console_key(
        &self,
        instance_id: &str,
        serial_port: i32,
        now: Instant,
    ) -> Option<&PushedKey> {
        self.serial_console_keys
            .get(&(instance_id.to_string(), serial_port))
            .filter(|key| !key.is_expired_at(now))
    }

    /// Forget every key whose lifetime has run out by `now`.
    ///
    /// This only materialises what the clock already made due — the keys were unusable from
    /// `expires_at` whether or not anyone called — so it runs on the write path, and reads
    /// ([`Self::active_ssh_key`]) apply the same cutoff without touching the maps.
    pub fn drop_expired_keys(&mut self, now: Instant) {
        self.pushed_keys.retain(|_, key| !key.is_expired_at(now));
        self.serial_console_keys
            .retain(|_, key| !key.is_expired_at(now));
    }

    /// Append a served call to the bounded request log.
    pub fn record_request(
        &mut self,
        request_id: &str,
        operation: &str,
        instance_id: &str,
        now: Instant,
    ) {
        if self.requests.len() >= MAX_RECORDED_REQUESTS {
            let overflow = self.requests.len() + 1 - MAX_RECORDED_REQUESTS;
            self.requests.drain(..overflow);
        }
        self.requests.push(RequestRecord {
            request_id: request_id.to_string(),
            operation: operation.to_string(),
            instance_id: instance_id.to_string(),
            at: now,
        });
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState>)
}
