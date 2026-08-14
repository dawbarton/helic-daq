//! Component-owned, discoverable parameter groups and their transactional store.
//!
//! Each group validates and stages its own values. [`ParamStore`] alone maps a
//! discovered index to `(group, local id)`, constructs the command address,
//! and accepts or rejects the staged update after queueing succeeds or fails.

use heapless::Vec;
use helic_proto::{ErrorCode, ParamType};

use crate::{CommandProducer, Payload, RtCommand, RtShared, SampleRate, DOMAIN_RIG};

mod groups;

pub use groups::{
    ControllerGroup, GeneratorGroup, PlatformGroup, RigGroup, TableGroup, TelemetryGroup,
};

/// Number of serialized Fourier coefficients for a selected harmonic count.
pub const fn coeff_count<const H: usize>() -> u16 {
    (1 + 2 * H) as u16
}
pub const MAX_GROUPS: usize = 8;
pub(crate) const MAX_CTRL_PARAMS: usize = 17;
pub(crate) const MAX_RIG_PARAMS: usize = 16;
pub(crate) const MAX_EXTRA_PARAMS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Scalar,
    Array(u16),
    Blob(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamDef {
    pub name: &'static str,
    pub ty: ParamType,
    pub count: u16,
    pub writable: bool,
    pub kind: ParamKind,
}

impl ParamDef {
    pub const fn read_only(name: &'static str, ty: ParamType, count: u16) -> Self {
        Self {
            name,
            ty,
            count,
            writable: false,
            kind: if count == 1 {
                ParamKind::Scalar
            } else {
                ParamKind::Array(count)
            },
        }
    }

    pub const fn writable(name: &'static str, ty: ParamType, count: u16) -> Self {
        Self {
            name,
            ty,
            count,
            writable: true,
            kind: if count == 1 {
                ParamKind::Scalar
            } else {
                ParamKind::Array(count)
            },
        }
    }

    pub const fn blob(name: &'static str, ty: ParamType, count: u16, maximum: u32) -> Self {
        Self {
            name,
            ty,
            count,
            writable: true,
            kind: ParamKind::Blob(maximum),
        }
    }
}

/// One experiment-owned, read-only scalar backed by an atomic word.
#[derive(Clone, Copy)]
pub struct ExtraParam {
    name: &'static str,
    ty: ParamType,
    value: &'static core::sync::atomic::AtomicU32,
    reset_on_diag: bool,
}

impl ExtraParam {
    pub const fn f32(name: &'static str, value: &'static core::sync::atomic::AtomicU32) -> Self {
        Self {
            name,
            ty: ParamType::F32,
            value,
            reset_on_diag: false,
        }
    }

    pub const fn u32(name: &'static str, value: &'static core::sync::atomic::AtomicU32) -> Self {
        Self {
            name,
            ty: ParamType::U32,
            value,
            reset_on_diag: false,
        }
    }

    pub const fn u32_event(
        name: &'static str,
        value: &'static core::sync::atomic::AtomicU32,
    ) -> Self {
        Self {
            name,
            ty: ParamType::U32,
            value,
            reset_on_diag: true,
        }
    }

    fn def(self) -> ParamDef {
        ParamDef::read_only(self.name, self.ty, 1)
    }

    fn get(self, out: &mut [u8]) {
        use core::sync::atomic::Ordering;
        out.copy_from_slice(&self.value.load(Ordering::Relaxed).to_le_bytes());
    }

    fn reset_diagnostic(self) {
        use core::sync::atomic::Ordering;
        if self.reset_on_diag {
            self.value.store(0, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamAction {
    None,
    Reboot,
    ResetDiagnostics,
}

pub enum Staged {
    Local(ParamAction),
    Rt(Payload),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandTarget {
    Core0,
    Rig,
    Program(u8),
}

pub trait ParamGroup {
    fn target(&self) -> CommandTarget {
        CommandTarget::Core0
    }

    fn params(&self) -> &[ParamDef];
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode>;

    /// Validate and stage without changing host-observable state.
    ///
    /// Returning `Err` must leave no pending state: the store cannot call
    /// [`reject`](Self::reject) when staging itself fails.
    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode>;

    fn accept(&mut self, id: u16);
    fn reject(&mut self, id: u16, returned: Option<Payload>);

    fn reset_diagnostics(&mut self) {}

    fn set_block(&mut self, _id: u16, _offset: u32, _data: &[u8]) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }

    fn stage_commit(&mut self, _id: u16, _len: u32) -> Result<Staged, ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
}

struct GroupEntry {
    group: &'static mut dyn ParamGroup,
    target: CommandTarget,
}

/// Fixed-capacity walk over component-owned parameter groups.
pub struct ParamStore {
    commands: CommandProducer,
    shared: &'static RtShared,
    sample_rate: SampleRate,
    entries: Vec<GroupEntry, MAX_GROUPS>,
    validated: bool,
}

impl ParamStore {
    pub const fn new(
        commands: CommandProducer,
        shared: &'static RtShared,
        sample_rate: SampleRate,
    ) -> Self {
        Self {
            commands,
            shared,
            sample_rate,
            entries: Vec::new(),
            validated: false,
        }
    }

    /// Register a statically allocated group, capturing its target exactly once.
    pub fn push(&mut self, group: &'static mut dyn ParamGroup) {
        assert!(!self.validated, "cannot add groups after validation");
        let entry = GroupEntry {
            target: group.target(),
            group,
        };
        assert!(
            self.entries.push(entry).is_ok(),
            "parameter group capacity exceeded"
        );
    }

    /// Validate the complete composition before the control task starts.
    pub fn validate(&mut self, program_domains: &[u8]) {
        assert!(!self.validated, "parameter store validated twice");
        let count = self.total_count();
        assert!(
            count <= u16::MAX as usize,
            "parameter registry exceeds protocol index range"
        );

        for (index, def) in (0..count).map(|index| (index, self.def_unchecked(index).unwrap())) {
            assert!(
                def.name.is_ascii() && def.name.len() <= helic_proto::payload::MAX_PARAM_NAME_LEN,
                "parameter name is non-ASCII or too long"
            );
            let encoded_len = def.name.len() + 5;
            assert!(
                encoded_len <= helic_proto::MAX_PAYLOAD - 4,
                "parameter definition cannot fit in one discovery page"
            );
            if let ParamKind::Blob(maximum) = def.kind {
                assert!(
                    maximum <= u16::MAX as u32,
                    "blob maximum cannot be represented by discovery"
                );
            }
            for previous in 0..index {
                assert_ne!(
                    def.name,
                    self.def_unchecked(previous).unwrap().name,
                    "parameter names must be unique"
                );
            }
        }

        for (index, domain) in program_domains.iter().copied().enumerate() {
            assert_ne!(
                domain, DOMAIN_RIG,
                "programme domain zero is reserved for the rig"
            );
            for previous in &program_domains[..index] {
                assert_ne!(domain, *previous, "programme domains must be unique");
            }
            let claims = self
                .entries
                .iter()
                .filter(|entry| entry.target == CommandTarget::Program(domain))
                .count();
            assert_eq!(claims, 1, "each programme domain needs exactly one group");
        }
        for entry in &self.entries {
            if let CommandTarget::Program(domain) = entry.target {
                assert!(
                    program_domains.contains(&domain),
                    "parameter group targets an unclaimed programme domain"
                );
            }
        }
        assert!(
            self.entries
                .iter()
                .filter(|entry| entry.target == CommandTarget::Rig)
                .count()
                <= 1,
            "only one parameter group may target the rig"
        );
        self.validated = true;
    }

    pub fn shared(&self) -> &'static RtShared {
        self.shared
    }

    pub fn count(&self) -> usize {
        self.assert_validated();
        self.total_count()
    }

    pub fn def(&self, index: usize) -> Option<ParamDef> {
        self.assert_validated();
        self.def_unchecked(index)
    }

    pub fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode> {
        self.assert_validated();
        let (group, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        self.entries[group].group.get(id, out)
    }

    pub fn set(&mut self, index: usize, data: &[u8]) -> Result<ParamAction, ErrorCode> {
        self.assert_validated();
        let (group, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        let def = self.entries[group]
            .group
            .params()
            .get(id as usize)
            .copied()
            .ok_or(ErrorCode::BadIndex)?;
        if !def.writable {
            return Err(ErrorCode::ReadOnly);
        }
        if data.len() != def.ty.size() * def.count as usize {
            return Err(ErrorCode::BadLength);
        }
        let staged = self.entries[group].group.stage(id, data)?;
        self.finish_staged(group, id, staged)
    }

    pub fn set_block(&mut self, index: usize, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        self.assert_validated();
        let (group, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        self.entries[group].group.set_block(id, offset, data)
    }

    pub fn commit(&mut self, index: usize, len: u32) -> Result<(), ErrorCode> {
        self.assert_validated();
        let (group, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        let staged = self.entries[group].group.stage_commit(id, len)?;
        self.finish_staged(group, id, staged).map(|_| ())
    }

    pub const fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn finish_staged(
        &mut self,
        group: usize,
        id: u16,
        staged: Staged,
    ) -> Result<ParamAction, ErrorCode> {
        match staged {
            Staged::Local(action) => {
                self.entries[group].group.accept(id);
                if action == ParamAction::ResetDiagnostics {
                    for entry in &mut self.entries {
                        entry.group.reset_diagnostics();
                    }
                    #[cfg(feature = "diag-max-command-burst")]
                    self.enqueue_max_command_burst()?;
                    Ok(ParamAction::None)
                } else {
                    Ok(action)
                }
            }
            Staged::Rt(payload) => {
                let domain = match self.entries[group].target {
                    CommandTarget::Rig => DOMAIN_RIG,
                    CommandTarget::Program(domain) => domain,
                    CommandTarget::Core0 => {
                        self.entries[group].group.reject(id, Some(payload));
                        return Err(ErrorCode::BadIndex);
                    }
                };
                let command = RtCommand {
                    domain,
                    id,
                    payload,
                };
                match self.commands.enqueue(command) {
                    Ok(()) => {
                        self.entries[group].group.accept(id);
                        Ok(ParamAction::None)
                    }
                    Err(returned) => {
                        self.entries[group].group.reject(id, Some(returned.payload));
                        Err(ErrorCode::Busy)
                    }
                }
            }
        }
    }

    #[cfg(feature = "diag-max-command-burst")]
    fn enqueue_max_command_burst(&mut self) -> Result<(), ErrorCode> {
        if self.commands.capacity() - self.commands.len() < crate::COMMANDS_PER_TICK {
            return Err(ErrorCode::Busy);
        }
        for _ in 0..crate::COMMANDS_PER_TICK {
            let result = self.commands.enqueue(RtCommand {
                domain: crate::DOMAIN_GENERATOR,
                id: crate::command_id::generator::DIAGNOSTIC_BURST,
                payload: Payload::F32(0.0),
            });
            debug_assert!(result.is_ok());
        }
        Ok(())
    }

    fn assert_validated(&self) {
        assert!(self.validated, "parameter store used before validation");
    }

    fn total_count(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.group.params().len())
            .sum()
    }

    fn def_unchecked(&self, index: usize) -> Option<ParamDef> {
        let (group, id) = self.locate(index)?;
        self.entries[group].group.params().get(id as usize).copied()
    }

    /// The sole global-to-local index arithmetic in the runtime.
    fn locate(&self, index: usize) -> Option<(usize, u16)> {
        let mut base = 0;
        for (group, entry) in self.entries.iter().enumerate() {
            let count = entry.group.params().len();
            if index < base + count {
                return Some((group, (index - base) as u16));
            }
            base += count;
        }
        None
    }
}

pub trait ParamRegistry {
    fn count(&self) -> usize;
    fn def(&self, index: usize) -> Option<ParamDef>;
    fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode>;
    fn set(&mut self, index: usize, data: &[u8]) -> Result<(), ErrorCode>;
    fn set_block(&mut self, _index: usize, _offset: u32, _data: &[u8]) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn commit(&mut self, _index: usize, _len: u32) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn sample_rate(&self) -> SampleRate;
}

impl ParamRegistry for ParamStore {
    fn count(&self) -> usize {
        ParamStore::count(self)
    }

    fn def(&self, index: usize) -> Option<ParamDef> {
        ParamStore::def(self, index)
    }

    fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode> {
        ParamStore::get(self, index, out)
    }

    fn set(&mut self, index: usize, data: &[u8]) -> Result<(), ErrorCode> {
        ParamStore::set(self, index, data).map(|_| ())
    }

    fn set_block(&mut self, index: usize, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        ParamStore::set_block(self, index, offset, data)
    }

    fn commit(&mut self, index: usize, len: u32) -> Result<(), ErrorCode> {
        ParamStore::commit(self, index, len)
    }

    fn sample_rate(&self) -> SampleRate {
        ParamStore::sample_rate(self)
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};
    use std::boxed::Box;

    use heapless::spsc::Queue;
    use helic_core::controller::{Controller, PassThrough};
    use helic_core::generator::FourierCoeffs;
    use helic_core::{DoubleBuffer, TableBuffer};

    use super::*;
    use crate::{CommandConsumer, Rig, COMMAND_QUEUE_LEN};

    const TEST_HARMONICS: usize = crate::DEFAULT_HARMONICS;
    const COEFF_COUNT: u16 = coeff_count::<TEST_HARMONICS>();

    static EXTRA_VALUE: AtomicU32 = AtomicU32::new(0);
    static EXTRA_EVENT: AtomicU32 = AtomicU32::new(0);
    static EXTRAS: &[ExtraParam] = &[
        ExtraParam::f32("extra", &EXTRA_VALUE),
        ExtraParam::u32_event("extra_event", &EXTRA_EVENT),
    ];

    struct TestRig;

    impl Rig for TestRig {
        const INPUTS: &'static [(&'static str, &'static str)] = &[("adc0", "V")];
        const ACTUATORS: &'static [(&'static str, &'static str)] = &[("out", "V")];

        fn init(&mut self) {}
        fn measure(&mut self, _values: &mut [f32]) {}
        fn actuate(&mut self, _outputs: &[f32]) {}
        fn prepare_reboot(&mut self, _step: u8) -> bool {
            true
        }
        fn param_names() -> &'static [&'static str] {
            &["rig_gain"]
        }
        fn param_defaults() -> &'static [f32] {
            &[1.0]
        }
    }

    fn store() -> (
        ParamStore,
        CommandConsumer,
        helic_core::ActiveTable,
        crate::ActiveCoeffs,
    ) {
        let queue = Box::leak(Box::new(Queue::<RtCommand, COMMAND_QUEUE_LEN>::new()));
        let (commands, consumer) = queue.split();
        let shared = Box::leak(Box::new(RtShared::new()));
        let (table, active_table) = Box::leak(Box::new(TableBuffer::new())).split();
        let (target, active_target) = Box::leak(Box::new(DoubleBuffer::from_banks(
            FourierCoeffs::zero(),
            FourierCoeffs::zero(),
        )))
        .split();
        let (forcing, _active_forcing) = Box::leak(Box::new(DoubleBuffer::from_banks(
            FourierCoeffs::zero(),
            FourierCoeffs::zero(),
        )))
        .split();

        let mut store = ParamStore::new(commands, shared, SampleRate::Hz8000);
        store.push(Box::leak(Box::new(PlatformGroup::new(
            shared,
            SampleRate::Hz8000,
            "0.1.0 test",
            "test-rig",
        ))));
        store.push(Box::leak(Box::new(GeneratorGroup::new(
            target,
            forcing,
            SampleRate::Hz8000,
        ))));
        store.push(Box::leak(Box::new(TableGroup::new(
            table,
            SampleRate::Hz8000,
        ))));
        store.push(Box::leak(Box::new(ControllerGroup::new(
            &PassThrough,
            TestRig::INPUTS.len(),
        ))));
        store.push(Box::leak(Box::new(RigGroup::<TestRig>::new())));
        store.push(Box::leak(Box::new(TelemetryGroup::new(EXTRAS))));
        store.validate(<crate::StandardProgram<PassThrough> as crate::Program>::DOMAINS);
        (store, consumer, active_table, active_target)
    }

    fn index(store: &ParamStore, name: &str) -> usize {
        (0..store.count())
            .find(|index| store.def(*index).unwrap().name == name)
            .unwrap()
    }

    #[test]
    fn registry_preserves_the_complete_discovered_set() {
        let (store, _commands, _table, _target) = store();
        let mut names: std::vec::Vec<_> = (0..store.count())
            .map(|index| store.def(index).unwrap().name)
            .collect();
        names.sort_unstable();
        let mut expected = std::vec![
            "arm",
            "clock_jitter",
            "cmd_backlog_max",
            "ctrl_reset",
            "diag_reset",
            "experiment",
            "extra",
            "extra_event",
            "firmware",
            "forcing_coeffs",
            "freq",
            "loop_time_last",
            "loop_time_max",
            "mcu_reboot",
            "overruns",
            "records_dropped",
            "rig_gain",
            "safety",
            "sample_freq",
            "t_actuate_max",
            "t_measure_max",
            "t_rest_max",
            "table",
            "table_freq",
            "table_gain",
            "table_interp",
            "table_len",
            "table_mode",
            "table_mult",
            "table_phase",
            "table_trigger",
            "target_coeffs",
            "tick_timeouts",
            "ticks",
            "wake_phase_max",
            "wake_phase_min",
        ];
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn generator_group_discovers_its_const_generic_harmonic_count() {
        const HARMONICS: usize = 3;
        let (target, _active_target) = Box::leak(Box::new(DoubleBuffer::from_banks(
            FourierCoeffs::<HARMONICS>::zero(),
            FourierCoeffs::<HARMONICS>::zero(),
        )))
        .split();
        let (forcing, _active_forcing) = Box::leak(Box::new(DoubleBuffer::from_banks(
            FourierCoeffs::<HARMONICS>::zero(),
            FourierCoeffs::<HARMONICS>::zero(),
        )))
        .split();
        let group = GeneratorGroup::<HARMONICS>::new(target, forcing, SampleRate::Hz8000);

        assert_eq!(group.params()[1].count, 1 + 2 * HARMONICS as u16);
        assert_eq!(group.params()[2].count, 1 + 2 * HARMONICS as u16);
    }

    #[test]
    fn global_walk_routes_and_accepts_only_after_enqueue() {
        let (mut store, mut commands, _table, _target) = store();
        let frequency = index(&store, "freq");
        store.set(frequency, &20.0f32.to_le_bytes()).unwrap();
        let command = commands.dequeue().unwrap();
        assert_eq!(command.domain, crate::DOMAIN_GENERATOR);
        assert_eq!(command.id, crate::command_id::generator::SET_INCREMENT);

        let mut out = [0; 4];
        store.get(frequency, &mut out).unwrap();
        assert_eq!(f32::from_le_bytes(out), 20.0);
    }

    #[test]
    fn table_group_local_ids_are_wire_command_ids() {
        let (mut store, mut commands, _table, _target) = store();
        let frequency = index(&store, "table_freq");
        store.set(frequency, &20.0_f32.to_le_bytes()).unwrap();
        assert!(matches!(
            commands.dequeue(),
            Some(RtCommand {
                domain: crate::DOMAIN_TABLE,
                id: crate::command_id::table::SET_INCREMENT,
                payload: Payload::U32(_),
            })
        ));
    }

    struct AdjustableController;

    impl Controller for AdjustableController {
        fn tick(&mut self, _inputs: &[f32], reference: f32, _dt: f32) -> f32 {
            reference
        }

        fn param_names() -> &'static [&'static str] {
            &["adjustment"]
        }

        fn param_value(&self, id: u16) -> Option<f32> {
            (id == 0).then_some(1.0)
        }
    }

    #[test]
    fn controller_group_local_ids_are_wire_command_ids() {
        let queue = Box::leak(Box::new(Queue::<RtCommand, COMMAND_QUEUE_LEN>::new()));
        let (producer, mut consumer) = queue.split();
        let shared = Box::leak(Box::new(RtShared::new()));
        let mut store = ParamStore::new(producer, shared, SampleRate::Hz8000);
        store.push(Box::leak(Box::new(ControllerGroup::new(
            &AdjustableController,
            1,
        ))));
        store.validate(&[crate::DOMAIN_CONTROLLER]);

        store.set(0, &1_u32.to_le_bytes()).unwrap();
        assert!(matches!(
            consumer.dequeue(),
            Some(RtCommand {
                domain: crate::DOMAIN_CONTROLLER,
                id: crate::command_id::controller::RESET,
                payload: Payload::Unit,
            })
        ));

        store.set(1, &2.0_f32.to_le_bytes()).unwrap();
        assert!(matches!(
            consumer.dequeue(),
            Some(RtCommand {
                domain: crate::DOMAIN_CONTROLLER,
                id: 1,
                payload: Payload::F32(2.0),
            })
        ));
    }

    #[test]
    fn full_queue_rejects_scalar_and_buffer_transactionally() {
        let (mut store, mut commands, _table, _target) = store();
        let frequency = index(&store, "freq");
        while store.set(frequency, &20.0f32.to_le_bytes()).is_ok() {}
        assert_eq!(
            store.set(frequency, &21.0f32.to_le_bytes()),
            Err(ErrorCode::Busy)
        );
        let mut out = [0; 4];
        store.get(frequency, &mut out).unwrap();
        assert_eq!(f32::from_le_bytes(out), 20.0);

        let target = index(&store, "target_coeffs");
        let coefficients = [0; COEFF_COUNT as usize * 4];
        assert_eq!(store.set(target, &coefficients), Err(ErrorCode::Busy));
        commands.dequeue().unwrap();
        assert_eq!(store.set(target, &coefficients), Ok(ParamAction::None));
    }

    #[test]
    fn diagnostic_reset_broadcasts_without_touching_live_state() {
        let (mut store, _commands, _table, _target) = store();
        store.shared.live.ticks.store(17, Ordering::Relaxed);
        store
            .shared
            .diagnostics
            .overruns
            .store(9, Ordering::Relaxed);
        EXTRA_EVENT.store(11, Ordering::Relaxed);
        let reset = index(&store, "diag_reset");
        store.set(reset, &1_u32.to_le_bytes()).unwrap();
        assert_eq!(store.shared.live.ticks.load(Ordering::Relaxed), 17);
        assert_eq!(store.shared.diagnostics.overruns.load(Ordering::Relaxed), 0);
        assert_eq!(EXTRA_EVENT.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn coefficient_buffer_activates_the_complete_staged_value() {
        let (mut store, mut commands, _table, mut active_target) = store();
        let target = index(&store, "target_coeffs");
        let coefficients = FourierCoeffs {
            mean: 0.25,
            a: core::array::from_fn(|index| index as f32 + 1.0),
            b: core::array::from_fn(|index| -(index as f32) - 1.0),
        };
        let mut bytes = [0; COEFF_COUNT as usize * 4];
        bytes[..4].copy_from_slice(&coefficients.mean.to_le_bytes());
        for harmonic in 0..TEST_HARMONICS {
            bytes[4 + 4 * harmonic..8 + 4 * harmonic]
                .copy_from_slice(&coefficients.a[harmonic].to_le_bytes());
            let offset = 4 + 4 * (TEST_HARMONICS + harmonic);
            bytes[offset..offset + 4].copy_from_slice(&coefficients.b[harmonic].to_le_bytes());
        }
        store.set(target, &bytes).unwrap();
        let Some(RtCommand {
            domain: crate::DOMAIN_GENERATOR,
            id: crate::command_id::generator::SET_TARGET,
            payload: Payload::Buffer(token),
        }) = commands.dequeue()
        else {
            panic!("target write was not routed to its buffer activation");
        };
        active_target.activate(token);
        assert_eq!(active_target.get().mean, coefficients.mean);
        assert_eq!(active_target.get().a, coefficients.a);
        assert_eq!(active_target.get().b, coefficients.b);
    }

    #[test]
    fn failing_stage_leaves_no_coefficient_pending_state() {
        let (mut store, _commands, _table, _target) = store();
        let target = index(&store, "target_coeffs");
        let mut invalid = [0; COEFF_COUNT as usize * 4];
        invalid[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(store.set(target, &invalid), Err(ErrorCode::BadValue));
        assert_eq!(
            store.set(target, &[0; COEFF_COUNT as usize * 4]),
            Ok(ParamAction::None)
        );
    }

    #[test]
    fn table_length_is_published_only_after_core_one_activation() {
        let (mut store, mut commands, mut active_table, _target) = store();
        let table = index(&store, "table");
        let table_len = index(&store, "table_len");
        let bytes: std::vec::Vec<_> = [1.0_f32, 2.0, 3.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        store.set_block(table, 0, &bytes).unwrap();
        store.commit(table, 3).unwrap();

        let mut out = [0; 2];
        store.get(table_len, &mut out).unwrap();
        assert_eq!(u16::from_le_bytes(out), 0);

        let Some(RtCommand {
            domain: crate::DOMAIN_TABLE,
            id: crate::command_id::table::ACTIVATE,
            payload: Payload::Buffer(token),
        }) = commands.dequeue()
        else {
            panic!("table commit was not routed to activation");
        };
        active_table.activate(token);
        store
            .shared
            .live
            .active_table_len
            .store(active_table.get().len() as u32, Ordering::Relaxed);
        store.get(table_len, &mut out).unwrap();
        assert_eq!(u16::from_le_bytes(out), 3);
    }

    #[test]
    fn full_queue_returns_table_token_to_its_group() {
        let (mut store, _commands, _table, _target) = store();
        let table = index(&store, "table");
        let frequency = index(&store, "freq");
        store
            .set_block(
                table,
                0,
                &[1.0_f32.to_le_bytes(), 2.0_f32.to_le_bytes()].concat(),
            )
            .unwrap();
        while store.set(frequency, &20.0_f32.to_le_bytes()).is_ok() {}
        assert_eq!(store.commit(table, 2), Err(ErrorCode::Busy));
        assert!(store.set_block(table, 0, &3.0_f32.to_le_bytes()).is_ok());
    }

    #[test]
    fn rig_group_maps_global_index_to_local_command_id() {
        let (mut store, mut commands, _table, _target) = store();
        let rig_gain = index(&store, "rig_gain");
        store.set(rig_gain, &2.5_f32.to_le_bytes()).unwrap();
        assert!(matches!(
            commands.dequeue(),
            Some(RtCommand {
                domain: DOMAIN_RIG,
                id: 0,
                payload: Payload::F32(2.5),
            })
        ));
        let mut out = [0; 4];
        store.get(rig_gain, &mut out).unwrap();
        assert_eq!(f32::from_le_bytes(out), 2.5);
    }

    struct BadCore0Group {
        rejected: &'static AtomicU32,
    }

    const BAD_PARAM: &[ParamDef] = &[ParamDef::writable("bad", ParamType::U32, 1)];

    impl ParamGroup for BadCore0Group {
        fn params(&self) -> &[ParamDef] {
            BAD_PARAM
        }
        fn get(&self, _id: u16, _out: &mut [u8]) -> Result<usize, ErrorCode> {
            Ok(0)
        }
        fn stage(&mut self, _id: u16, _data: &[u8]) -> Result<Staged, ErrorCode> {
            Ok(Staged::Rt(Payload::U32(1)))
        }
        fn accept(&mut self, _id: u16) {}
        fn reject(&mut self, _id: u16, returned: Option<Payload>) {
            if matches!(returned, Some(Payload::U32(1))) {
                self.rejected.store(1, Ordering::Relaxed);
            }
        }
    }

    #[test]
    fn core_zero_group_rt_payload_is_rejected_and_returned() {
        static REJECTED: AtomicU32 = AtomicU32::new(0);
        let queue = Box::leak(Box::new(Queue::<RtCommand, COMMAND_QUEUE_LEN>::new()));
        let (commands, _consumer) = queue.split();
        let shared = Box::leak(Box::new(RtShared::new()));
        let mut store = ParamStore::new(commands, shared, SampleRate::Hz8000);
        store.push(Box::leak(Box::new(BadCore0Group {
            rejected: &REJECTED,
        })));
        store.validate(&[]);
        assert_eq!(store.set(0, &1_u32.to_le_bytes()), Err(ErrorCode::BadIndex));
        assert_eq!(REJECTED.load(Ordering::Relaxed), 1);
    }

    struct TargetGroup {
        target: CommandTarget,
        defs: &'static [ParamDef],
    }

    impl ParamGroup for TargetGroup {
        fn target(&self) -> CommandTarget {
            self.target
        }
        fn params(&self) -> &[ParamDef] {
            self.defs
        }
        fn get(&self, _id: u16, _out: &mut [u8]) -> Result<usize, ErrorCode> {
            Ok(0)
        }
        fn stage(&mut self, _id: u16, _data: &[u8]) -> Result<Staged, ErrorCode> {
            Ok(Staged::Local(ParamAction::None))
        }
        fn accept(&mut self, _id: u16) {}
        fn reject(&mut self, _id: u16, _returned: Option<Payload>) {}
    }

    fn empty_store() -> ParamStore {
        let queue = Box::leak(Box::new(Queue::<RtCommand, COMMAND_QUEUE_LEN>::new()));
        let (commands, _consumer) = queue.split();
        ParamStore::new(
            commands,
            Box::leak(Box::new(RtShared::new())),
            SampleRate::Hz8000,
        )
    }

    const TARGET_A: &[ParamDef] = &[ParamDef::read_only("target_a", ParamType::U32, 1)];
    const TARGET_B: &[ParamDef] = &[ParamDef::read_only("target_b", ParamType::U32, 1)];

    #[test]
    #[should_panic(expected = "each programme domain needs exactly one group")]
    fn validation_rejects_duplicate_programme_target() {
        let mut store = empty_store();
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Program(1),
            defs: TARGET_A,
        })));
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Program(1),
            defs: TARGET_B,
        })));
        store.validate(&[1]);
    }

    #[test]
    #[should_panic(expected = "programme domain zero is reserved for the rig")]
    fn validation_rejects_zero_programme_domain() {
        empty_store().validate(&[0]);
    }

    #[test]
    #[should_panic(expected = "each programme domain needs exactly one group")]
    fn validation_rejects_unreachable_programme_domain() {
        empty_store().validate(&[1]);
    }

    #[test]
    #[should_panic(expected = "unclaimed programme domain")]
    fn validation_rejects_group_for_unknown_programme_domain() {
        let mut store = empty_store();
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Program(7),
            defs: TARGET_A,
        })));
        store.validate(&[]);
    }

    #[test]
    #[should_panic(expected = "only one parameter group may target the rig")]
    fn validation_rejects_multiple_rig_groups() {
        let mut store = empty_store();
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Rig,
            defs: TARGET_A,
        })));
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Rig,
            defs: TARGET_B,
        })));
        store.validate(&[]);
    }

    const LONG_NAME: &[ParamDef] = &[ParamDef::read_only(
        "parameter_name_far_too_long",
        ParamType::U32,
        1,
    )];

    #[test]
    #[should_panic(expected = "parameter name is non-ASCII or too long")]
    fn validation_rejects_definition_that_cannot_be_discovered() {
        let mut store = empty_store();
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Core0,
            defs: LONG_NAME,
        })));
        store.validate(&[]);
    }

    const OVERSIZED_BLOB: &[ParamDef] = &[ParamDef::blob(
        "blob",
        ParamType::F32,
        1,
        u16::MAX as u32 + 1,
    )];

    #[test]
    #[should_panic(expected = "blob maximum cannot be represented by discovery")]
    fn validation_rejects_blob_maximum_outside_wire_range() {
        let mut store = empty_store();
        store.push(Box::leak(Box::new(TargetGroup {
            target: CommandTarget::Core0,
            defs: OVERSIZED_BLOB,
        })));
        store.validate(&[]);
    }

    #[test]
    fn table_group_discovers_its_const_generic_capacity() {
        let (staging, _active) = Box::leak(Box::new(helic_core::TableBuffer::<8>::new())).split();
        let group = TableGroup::<8>::new(staging, SampleRate::Hz8000);
        assert_eq!(group.params()[0].count, 8);
        assert_eq!(group.params()[0].kind, ParamKind::Blob(8));
    }
}
