use ab_riscv_interpreter::prelude::*;
use ab_riscv_primitives::prelude::*;
use chacha20::ChaCha8Rng;
use chacha20::rand_core::{Rng, SeedableRng};
use core::ops::ControlFlow;
use std::collections::BTreeMap;

/// Bit index of `mstatus.MIE`
const MSTATUS_MIE_BIT: u8 = 3;
/// Bit index of `mstatus.MPIE`
const MSTATUS_MPIE_BIT: u8 = 7;
/// Bit index of `mstatus.VS[0]` (2-bit field, `mstatus.VS[1]` is `MSTATUS_VS_BIT + 1`)
const MSTATUS_VS_BIT: u8 = 9;
/// Bit index of `mstatus.MPP[0]` (2-bit field, `mstatus.MPP[1]` is `MSTATUS_MPP_BIT + 1`)
const MSTATUS_MPP_BIT: u8 = 11;

/// Mask a raw `mstatus` value down to the fields this M-mode-only, F-less, S/U-less core actually
/// implements.
///
/// Per the mstatus field table: MIE and MPIE are real read/write bits; VS is a real 2-bit WARL
/// field (this core has vector support, via ZveXx); MPP is hardwired to M (`0b11`) since M is the
/// only implemented privilege mode - there is no lower mode to return to, or to hold a reservation
/// for; SD is computed from VS (FS and XS are always 0, since F and other user extensions with XS
/// state aren't implemented). Every other field (SIE/SPIE/UBE/SPP/FS/XS/MPRV/SUM/MXR/TVM/TW/TSR
/// and the WPRI ranges) is read-only zero, since S-mode and U-mode are both unimplemented.
pub(crate) fn mask_mstatus<Reg>(value: Reg::Type) -> Reg::Type
where
    Reg: Register,
{
    let one = Reg::Type::from(1u8);
    let mie = value & (one << MSTATUS_MIE_BIT);
    let mpie = value & (one << MSTATUS_MPIE_BIT);
    let vs = (value >> MSTATUS_VS_BIT) & Reg::Type::from(0b11u8);
    let mpp = Reg::Type::from(0b11u8) << MSTATUS_MPP_BIT;
    let sd = if vs == Reg::Type::from(0b11u8) {
        one << (Reg::XLEN - 1)
    } else {
        Reg::Type::default()
    };
    mie | mpie | (vs << MSTATUS_VS_BIT) | mpp | sd
}

/// `misa` bit of the `A` extension
pub(crate) const MISA_A: u32 = 1 << 0;
/// `misa` bit of the `B` extension
pub(crate) const MISA_B: u32 = 1 << 1;
/// `misa` bit of the `I` base ISA
pub(crate) const MISA_I: u32 = 1 << 8;
/// `misa` bit of the `M` extension
pub(crate) const MISA_M: u32 = 1 << 12;

/// Properties of the core under test that are not captured by its instruction set
#[derive(Debug, Clone, Copy)]
pub(crate) struct CoreConfig {
    /// Single-letter extension bits reported in `misa` (see `MISA_*` constants)
    pub(crate) misa_extensions: u32,
    /// Whether Zkr is implemented, which is what makes `seed` and `mseccfg`/`mseccfgh` CSRs exist
    pub(crate) zkr: bool,
    /// Whether Zca is implemented. Without it IALIGN=32, so `mepc[1:0]` is always zero rather than
    /// just `mepc[0]`.
    pub(crate) zca: bool,
}

/// Access to the core under test from within instruction implementations
pub(crate) trait CoreEnv {
    /// Configuration of the core under test
    fn core_config(&self) -> &CoreConfig;

    /// Suppress the `minstret` increment of the current instruction, since it wrote `minstret`
    fn suppress_minstret_increment(&mut self);
}

/// Fixed `misa` value for the core.
///
/// `MISA_CSR_IMPLEMENTED: false` (per `sail.json`'s `mutable_misa: false` for every extension) only
/// means misa isn't *writable* - every write is WARL-ignored - not that it reads back all zero. MXL
/// (top 2 bits) reports the hart's fixed base ISA width, and each extension-letter bit reports
/// whether that single-letter extension is actually implemented; both stay readable and accurate
/// regardless of writability (verified against the reference model). `Z*` sub-extensions have no
/// misa bit of their own, neither do `Zve*` (only the full `V` extension sets `misa.V`).
fn misa_value<Reg>(extensions: u32) -> Reg::Type
where
    Reg: Register,
{
    let mxl = Reg::Type::from(u8::from(Reg::XLEN == 64) + 1);
    (mxl << (Reg::XLEN - 2)) | Reg::Type::from(extensions)
}

/// Whether a CSR index is one of `mhpmevent3..31`, `mhpmcounter3..31` or, on RV32,
/// `mhpmcounter3h..31h`.
///
/// The privileged spec requires all of them to exist in M-mode, but since no HPM counters are
/// implemented (HPM_COUNTER_EN is all false), they are read-only zero.
pub(crate) fn is_hpm_csr<Reg>(csr_index: u16) -> bool
where
    Reg: Register,
{
    matches!(csr_index, 0x323..=0x33f | 0xb03..=0xb1f)
        || (Reg::XLEN == 32 && matches!(csr_index, 0xb83..=0xb9f))
}

/// Execution environment of the core under test
pub(crate) struct TestEnv<Hart>
where
    Hart: VectorHartConfig,
{
    csrs: BTreeMap<u16, <Hart::Reg as Register>::Type>,
    vregs: VectorRegisterFile<Hart>,
    reservation: Option<<Hart::Reg as Register>::Type>,
    entropy_source: ChaCha8Rng,
    core_config: CoreConfig,
    /// Whether an instruction was executed (or trapped) since the last [`TestEnv::finish_step()`]
    step_pending: bool,
    /// Whether the pending step retired an instruction rather than trapping
    retire_pending: bool,
    /// Whether the pending step wrote `minstret`/`minstreth`, which suppresses its increment
    minstret_written: bool,
}

impl<Reg, Hart> TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
    Self: VectorRegistersExt,
{
    /// Create a new instance with all CSRs the tests expect to exist initialized
    pub(crate) fn new(core_config: CoreConfig) -> Self {
        let zero = Reg::Type::default();

        let mut csrs = BTreeMap::new();
        // Machine ID registers (MRO): 0 is a legal "not implemented"/"non-commercial" value for
        // all four per spec, matching MARCHID_IMPLEMENTED/MIMPID_IMPLEMENTED (both false) and
        // VENDOR_ID_BANK/VENDOR_ID_OFFSET (both 0) in the DUT config. There's only ever one hart,
        // so mhartid is 0 too.
        csrs.insert(MCsr::Mvendorid as u16, zero);
        csrs.insert(MCsr::Marchid as u16, zero);
        csrs.insert(MCsr::Mimpid as u16, zero);
        csrs.insert(MCsr::Mhartid as u16, zero);
        // Vector CSRs
        csrs.insert(VectorCsr::Vstart.to_csr_index(), zero);
        csrs.insert(VectorCsr::Vxsat.to_csr_index(), zero);
        csrs.insert(VectorCsr::Vxrm.to_csr_index(), zero);
        csrs.insert(VectorCsr::Vcsr.to_csr_index(), zero);
        csrs.insert(VectorCsr::Vl.to_csr_index(), zero);
        csrs.insert(
            VectorCsr::Vtype.to_csr_index(),
            Reg::Type::from(1u8) << (Reg::XLEN - 1),
        );
        csrs.insert(
            VectorCsr::Vlenb.to_csr_index(),
            Reg::Type::from(Hart::VECTOR_LENGTHS.vlen.bytes()),
        );
        // Machine trap CSRs - zero-initialized, mtvec must be written by test boot code before any
        // trap can be taken. mstatus is the exception: mask_mstatus() forces MPP to M even from an
        // all-zero input, which is also the correct reset value (M is the only implemented mode).
        csrs.insert(MCsr::Mstatus as u16, mask_mstatus::<Reg>(zero));
        // MISA_CSR_IMPLEMENTED is false, so misa is hardwired to the value from `misa_value()` -
        // notably signaling no H (bit 7), which the ACT4 framework's own trap handler (under
        // STANDARD_SM_SUPPORTED) reads unconditionally on every M-mode trap to decide whether to
        // also save mtval2/mtinst
        csrs.insert(
            MCsr::Misa as u16,
            misa_value::<Reg>(core_config.misa_extensions),
        );
        // MEI_INTR_IMPL, MSI_INTR_IMPL and MTI_INTR_IMPL are all false, and there is no S/U-mode,
        // so every interrupt-enable and interrupt-pending bit is read-only zero, see
        // instruction.rs's prepare_csr_write
        csrs.insert(MCsr::Mie as u16, zero);
        csrs.insert(MCsr::Mtvec as u16, zero);
        csrs.insert(MCsr::Mscratch as u16, zero);
        csrs.insert(MCsr::Mepc as u16, zero);
        csrs.insert(MCsr::Mcause as u16, zero);
        csrs.insert(MCsr::Mtval as u16, zero);
        csrs.insert(MCsr::Mip as u16, zero);
        // mstatush only exists on RV32 (on RV64 its fields live in mstatus directly); the ACT4
        // framework's default M-mode boot (RVTEST_BOOT_TO_MMODE, under STANDARD_SM_SUPPORTED)
        // unconditionally clears it unless SM1P11P0_SUPPORTED is defined, which this core does
        // not define
        if Reg::XLEN == 32 {
            csrs.insert(MCsr::Mstatush as u16, zero);
        }
        // mseccfg (and mseccfgh, RV32 only) only exist when Zkr is implemented (it gates
        // SSEED/USEED access to the seed CSR), since neither Smepmp (MML/MMWP/RLB) nor any other
        // extension that would need them is. There's no S/U mode for SSEED/USEED to grant access
        // to either, so every field of both halves is hardwired to 0 - see instruction.rs's
        // prepare_csr_write. Without Zkr, all three CSRs are absent and every access traps.
        if core_config.zkr {
            csrs.insert(MCsr::Mseccfg as u16, zero);
            if Reg::XLEN == 32 {
                csrs.insert(MCsr::Mseccfgh as u16, zero);
            }
            csrs.insert(SEED_CSR_INDEX, zero);
        }
        // mcycle/minstret (mcycleh/minstreth on RV32) always exist in M-mode, see
        // `TestEnv::finish_step()` for how they count. So do mhpmevent3..31 and mhpmcounter3..31
        // (mhpmcounter3h..31h on RV32), but with no HPM counters implemented they are read-only
        // zero, see `is_hpm_csr()`.
        csrs.insert(MCsr::Mcycle as u16, zero);
        csrs.insert(MCsr::Minstret as u16, zero);
        if Reg::XLEN == 32 {
            csrs.insert(MCsr::Mcycleh as u16, zero);
            csrs.insert(MCsr::Minstreth as u16, zero);
        }
        for csr_index in (0x323..=0x33f).chain(0xb03..=0xb1f) {
            csrs.insert(csr_index, zero);
        }
        if Reg::XLEN == 32 {
            for csr_index in 0xb83..=0xb9f {
                csrs.insert(csr_index, zero);
            }
        }
        // mcountinhibit and menvcfg/menvcfgh are deliberately NOT inserted here, every access to
        // them traps illegal in the reference model: sail-riscv's `is_CSR_accessible` gates
        // mcountinhibit behind `base.mcountinhibit.supported` (MCOUNTINHIBIT_IMPLEMENTED) and
        // menvcfg/menvcfgh behind `Ext_U`, both of which are `false` in this DUT's `sail.json`.
        //
        // mconfigptr must always be implemented (read-only); CONFIG_PTR_ADDRESS is 0 for this core
        csrs.insert(MCsr::Mconfigptr as u16, zero);

        let mut s = Self {
            csrs,
            vregs: VectorRegisterFile::default(),
            reservation: None,
            // Good enough for testing purposes
            entropy_source: ChaCha8Rng::from_seed([0; _]),
            core_config,
            step_pending: false,
            retire_pending: false,
            minstret_written: false,
        };
        s.initialize_vector_state();
        s
    }

    /// Mark the start of an instruction that is about to be executed, see [`Self::finish_step()`]
    pub(crate) fn begin_step(&mut self) {
        self.step_pending = true;
        self.retire_pending = true;
    }

    /// Update counters after the previous instruction, called before the next one is fetched.
    ///
    /// Matches the reference model: `mcycle` increments after every executed or trapped
    /// instruction, even one that wrote `mcycle` itself, while `minstret` only increments when an
    /// instruction retires without writing `minstret`. Neither can be inhibited, since
    /// MCOUNTINHIBIT_IMPLEMENTED is false.
    pub(crate) fn finish_step(&mut self) {
        if !self.step_pending {
            return;
        }
        self.increment_counter(MCsr::Mcycle, MCsr::Mcycleh);
        if self.retire_pending && !self.minstret_written {
            self.increment_counter(MCsr::Minstret, MCsr::Minstreth);
        }
        self.step_pending = false;
        self.retire_pending = false;
        self.minstret_written = false;
    }

    fn increment_counter(&mut self, low: MCsr, high: MCsr) {
        let low_value = self.csrs[&(low as u16)].as_u64();
        if Reg::XLEN == 32 {
            let high_value = self.csrs[&(high as u16)].as_u64();
            let value = ((high_value << 32) | low_value).wrapping_add(1);
            self.csrs
                .insert(low as u16, Reg::Type::truncate_from_u64(value));
            self.csrs
                .insert(high as u16, Reg::Type::truncate_from_u64(value >> 32));
        } else {
            self.csrs.insert(
                low as u16,
                Reg::Type::truncate_from_u64(low_value.wrapping_add(1)),
            );
        }
    }

    /// Dispatch a synchronous trap, returning the new PC (mtvec target).
    ///
    /// Writes `mepc`, `mcause`, `mtval`, then returns `mtvec & !0b11` (direct mode only -
    /// MTVEC_MODES: `[0]`). If mtvec is zero, the test never set it up, which means the test
    /// doesn't expect traps; return None so the caller can treat it as a hard error.
    pub(crate) fn take_trap<Cause>(
        &mut self,
        cause: Cause,
        epc: Reg::Type,
        tval: Reg::Type,
    ) -> Option<Reg::Type>
    where
        MCause: From<Cause>,
    {
        // A trapping instruction still takes a cycle, but doesn't retire
        self.step_pending = true;
        self.retire_pending = false;
        *self.csrs.get_mut(&(MCsr::Mepc as u16)).unwrap() = epc;
        *self.csrs.get_mut(&(MCsr::Mcause as u16)).unwrap() = MCause::from(cause).to_raw::<Reg>();
        *self.csrs.get_mut(&(MCsr::Mtval as u16)).unwrap() = tval;

        // Per spec, trap entry saves the current MIE into MPIE, then clears MIE (interrupts are
        // disabled while the trap handler runs); MPP would normally record the pre-trap privilege
        // mode, but that's already forced to M unconditionally since M is the only mode this core
        // implements.
        let one = Reg::Type::from(1u8);
        let mstatus = self.csrs[&(MCsr::Mstatus as u16)];
        let mie = (mstatus >> MSTATUS_MIE_BIT) & one;
        let mstatus = (mstatus & !(one << MSTATUS_MIE_BIT) & !(one << MSTATUS_MPIE_BIT))
            | (mie << MSTATUS_MPIE_BIT);
        *self.csrs.get_mut(&(MCsr::Mstatus as u16)).unwrap() = mstatus;

        let mtvec = self.csrs[&(MCsr::Mtvec as u16)];
        if mtvec == Reg::Type::default() {
            return None;
        }

        Some(mtvec & !Reg::Type::from(0b11u8))
    }

    /// Handle `mret`: restore `mstatus.MIE` from `MPIE` (setting `MPIE` to 1 per spec), returning
    /// the target PC (`mepc`). `MPP` stays forced to M (the only implemented privilege mode).
    pub(crate) fn return_from_trap(&mut self) -> Reg::Type {
        self.begin_step();
        let one = Reg::Type::from(1u8);
        let mstatus = self.csrs[&(MCsr::Mstatus as u16)];
        let mpie = (mstatus >> MSTATUS_MPIE_BIT) & one;
        let mstatus = (mstatus & !(one << MSTATUS_MIE_BIT) & !(one << MSTATUS_MPIE_BIT))
            | (mpie << MSTATUS_MIE_BIT)
            | (one << MSTATUS_MPIE_BIT);
        *self.csrs.get_mut(&(MCsr::Mstatus as u16)).unwrap() = mstatus;

        self.csrs[&(MCsr::Mepc as u16)]
    }
}

impl<Reg, Hart> CoreEnv for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
    fn core_config(&self) -> &CoreConfig {
        &self.core_config
    }

    fn suppress_minstret_increment(&mut self) {
        self.minstret_written = true;
    }
}

impl<Reg, Hart> Csrs<Reg> for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
    fn privilege_level(&self) -> PrivilegeLevel {
        PrivilegeLevel::Machine
    }

    fn read_csr(&self, csr_index: u16) -> Result<Reg::Type, CsrError> {
        self.csrs
            .get(&csr_index)
            .copied()
            .ok_or(CsrError::IllegalRead { csr_index })
    }

    fn write_csr(&mut self, csr_index: u16, value: Reg::Type) -> Result<(), CsrError> {
        let slot = self
            .csrs
            .get_mut(&csr_index)
            .ok_or(CsrError::IllegalWrite { csr_index })?;
        *slot = value;
        Ok(())
    }
}

impl<Reg, Hart> VectorRegisters for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
    type Hart = Hart;

    fn read_vregs(&self) -> &VectorRegisterFile<Self::Hart> {
        &self.vregs
    }

    fn write_vregs(&mut self) -> &mut VectorRegisterFile<Self::Hart> {
        &mut self.vregs
    }

    fn vector_instructions_allowed(&self) -> bool {
        // Vector instructions are illegal while `mstatus.VS` is Off
        let mstatus = self.csrs[&(MCsr::Mstatus as u16)];
        (mstatus >> MSTATUS_VS_BIT) & Reg::Type::from(0b11u8) != Reg::Type::default()
    }

    fn mark_vs_dirty(&mut self) {
        // HW_MSTATUS_VS_DIRTY_UPDATE is precise, so `mstatus.VS` becomes Dirty whenever vector
        // state is modified
        let mstatus = self.csrs[&(MCsr::Mstatus as u16)];
        self.csrs.insert(
            MCsr::Mstatus as u16,
            mask_mstatus::<Reg>(mstatus | (Reg::Type::from(0b11u8) << MSTATUS_VS_BIT)),
        );
    }
}

impl<Reg, Hart> VectorRegistersExt for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
}

impl<Reg, Hart> ReservationSet<Reg> for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
    fn reservation(&self) -> Option<Reg::Type> {
        self.reservation
    }

    fn set_reservation(&mut self, address: Reg::Type) {
        self.reservation = Some(address);
    }

    fn clear_reservation(&mut self) {
        self.reservation = None;
    }
}

impl<Reg, Hart> ZkrSeedSource for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
    fn poll_seed(&mut self) -> ZkrSeedPoll {
        let mut randomness = [0; _];
        self.entropy_source.fill_bytes(&mut randomness);
        ZkrSeedPoll::Es16(u16::from_le_bytes(randomness))
    }
}

impl<Reg, Hart, Regs, Memory, PC> SystemInstructionHandler<Reg, Regs, Memory, PC> for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
    PC: ProgramCounter<Reg::Type, Memory>,
    Self: VectorRegistersExt,
{
    fn handle_ecall(
        &mut self,
        _regs: &mut Regs,
        memory: &mut Memory,
        program_counter: &mut PC,
    ) -> Result<ControlFlow<()>, ExecutionError<Reg::Type>> {
        // `ecall` always traps in M-mode (there is no less-privileged mode to delegate to, and
        // TRAP_ON_ECALL_FROM_M is true for this core) - dispatch through the trap handler like any
        // other synchronous exception, rather than treating it as an illegal instruction. Per spec,
        // `mtval` is always zero for an environment call, regardless of any DUT config parameter.
        let epc = program_counter.old_pc(size_of::<u32>() as u8);
        let trap_pc = self
            .take_trap(
                MCauseException::MachineEnvironmentCall,
                epc,
                Reg::Type::default(),
            )
            .ok_or(ExecutionError::IllegalInstruction {
                address: PackedAddress::new(epc),
            })?;
        program_counter.set_pc(memory, trap_pc)
    }

    fn handle_ebreak(
        &mut self,
        _regs: &mut Regs,
        memory: &mut Memory,
        program_counter: &mut PC,
        instruction_size: u8,
    ) -> Result<ControlFlow<()>, ExecutionError<Reg::Type>> {
        // `ebreak`/`c.ebreak` always trap in M-mode (TRAP_ON_EBREAK is true for this core) -
        // dispatch through the trap handler like `ecall` above, rather than the default no-op.
        // Per REPORT_VA_IN_MTVAL_ON_BREAKPOINT (false for this core), `mtval` is zero.
        let epc = program_counter.old_pc(instruction_size);
        let trap_pc = self
            .take_trap(MCauseException::Breakpoint, epc, Reg::Type::default())
            .ok_or(ExecutionError::IllegalInstruction {
                address: PackedAddress::new(epc),
            })?;
        program_counter.set_pc(memory, trap_pc)
    }
}

impl<Reg, Hart> WrsHandler for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
}

impl<Reg, Hart> FenceIHandler for TestEnv<Hart>
where
    Reg: Register,
    Hart: VectorHartConfig<Reg = Reg>,
{
}
