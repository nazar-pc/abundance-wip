extern crate alloc;

use crate::instructions::isa::{IsaExtension, MAX_ISA_EXTENSIONS};
use crate::instructions::rv32::Rv32Instruction;
use crate::instructions::rv32::zce::zcmp::{ZcmpRegister, ZcmpUrlist};
use crate::instructions::rv64::Rv64Instruction;
use crate::instructions::rv64::a::Rv64AInstruction;
use crate::instructions::rv64::a::zaamo::Rv64ZaamoInstruction;
use crate::instructions::rv64::a::zalrsc::Rv64ZalrscInstruction;
use crate::instructions::rv64::b::Rv64BInstruction;
use crate::instructions::rv64::b::zba::Rv64ZbaInstruction;
use crate::instructions::rv64::b::zbb::{Rv64ZbbInstruction, Rv64ZbbZbkbSharedInstruction};
use crate::instructions::rv64::b::zbs::Rv64ZbsInstruction;
use crate::instructions::rv64::c::zca::Rv64ZcaInstruction;
use crate::instructions::rv64::d::Rv64D;
use crate::instructions::rv64::m::Rv64MInstruction;
use crate::instructions::rv64::m::zmmul::Rv64ZmmulInstruction;
use crate::instructions::rv64::zabha::Rv64ZabhaInstruction;
use crate::instructions::rv64::zacas::Rv64ZacasInstruction;
use crate::instructions::rv64::zalasr::Rv64ZalasrInstruction;
use crate::instructions::rv64::zce::zcb::{Rv64ZcbInstruction, Rv64ZcbOnlyInstruction};
use crate::instructions::rv64::zce::zcmp::{Rv64ZcmpInstruction, Rv64ZcmpOnlyInstruction};
use crate::instructions::rv64::zk::zbkb::Rv64ZbkbInstruction;
use crate::instructions::rv64::zk::zbkc::Rv64ZbkcInstruction;
use crate::instructions::rv64::zk::zbkx::Rv64ZbkxInstruction;
use crate::instructions::rv64::zk::zkn::Rv64ZknInstruction;
use crate::instructions::rv64::zk::zkn::zknd::{
    Rv64ZkndInstruction, Rv64ZkndKsRnum, Rv64ZkndZkneSharedInstruction,
};
use crate::instructions::rv64::zk::zkn::zkne::Rv64ZkneInstruction;
use crate::instructions::rv64::zk::zkn::zknh::Rv64ZknhInstruction;
use crate::instructions::utils::{I24, I24WithZeroedBits};
use crate::instructions::v::zvexx::ZveXxInstruction;
use crate::instructions::v::zvexx::arith::ZveXxArithInstruction;
use crate::instructions::v::zvexx::carry::ZveXxCarryInstruction;
use crate::instructions::v::zvexx::config::ZveXxConfigInstruction;
use crate::instructions::v::zvexx::fixed_point::ZveXxFixedPointInstruction;
use crate::instructions::v::zvexx::load::{LoadStoreNreg, Nf, SegVmNf, ZveXxLoadInstruction};
use crate::instructions::v::zvexx::mask::ZveXxMaskInstruction;
use crate::instructions::v::zvexx::muldiv::ZveXxMulDivInstruction;
use crate::instructions::v::zvexx::perm::ZveXxPermInstruction;
use crate::instructions::v::zvexx::reduction::ZveXxReductionInstruction;
use crate::instructions::v::zvexx::store::ZveXxStoreInstruction;
use crate::instructions::v::zvexx::widen_narrow::ZveXxWidenNarrowInstruction;
use crate::instructions::v::{Eew, Elen, V, VectorLengths, Vlen};
use crate::instructions::zawrs::ZawrsInstruction;
use crate::instructions::zicond::ZicondInstruction;
use crate::instructions::zicsr::ZicsrInstruction;
use crate::instructions::zifencei::ZifenceiInstruction;
use crate::instructions::zkr::ZkrInstruction;
use crate::instructions::zvbb::ZvbbInstruction;
use crate::instructions::zvbb::zvkb::ZvkbInstruction;
use crate::instructions::zvbc::ZvbcInstruction;
use crate::instructions::{Instruction, InstructionIsa, implements_extension};
use crate::registers::general_purpose::{EReg, Reg, Register};
use crate::registers::vector::VReg;
use ab_riscv_macros::instruction;
use alloc::vec::Vec;
use core::fmt;

#[instruction(
    inherit = [Rv64Instruction, Rv64MInstruction],
)]
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
enum TestMInstruction<Reg> {}

#[instruction]
const impl<Reg> Instruction for TestMInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    const ALIGNMENT: u8 = align_of::<u16>() as u8;

    type Reg = Reg;

    #[inline(always)]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

#[instruction]
impl<Reg, Cfg> InstructionIsa<Cfg> for TestMInstruction<Reg>
where
    Reg: Register<Type = u64>,
{
    const OWN_ISA_EXTENSIONS: &'static [IsaExtension] = &[];
}

#[instruction]
impl<Reg> fmt::Display for TestMInstruction<Reg>
where
    Reg: Register,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {}
    }
}

#[instruction(
    inherit = [Rv64Instruction, Rv64MInstruction, Rv64AInstruction, Rv64BInstruction, ZicsrInstruction, ZifenceiInstruction, Rv64ZcbInstruction, Rv64ZcmpInstruction],
)]
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
enum TestScalarInstruction<Reg> {}

#[instruction]
const impl<Reg> Instruction for TestScalarInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    const ALIGNMENT: u8 = align_of::<u16>() as u8;

    type Reg = Reg;

    #[inline(always)]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

#[instruction]
impl<Reg, Cfg> InstructionIsa<Cfg> for TestScalarInstruction<Reg>
where
    Reg: Register<Type = u64>,
{
    const OWN_ISA_EXTENSIONS: &'static [IsaExtension] = &[];
}

#[instruction]
impl<Reg> fmt::Display for TestScalarInstruction<Reg>
where
    Reg: Register,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {}
    }
}

#[instruction(
    inherit = [Rv64Instruction, Rv64ZacasInstruction, Rv64ZabhaInstruction, Rv64ZalasrInstruction, ZawrsInstruction, ZicondInstruction, ZkrInstruction, Rv64ZknInstruction],
)]
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
enum TestMiscInstruction<Reg> {}

#[instruction]
const impl<Reg> Instruction for TestMiscInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    const ALIGNMENT: u8 = align_of::<u16>() as u8;

    type Reg = Reg;

    #[inline(always)]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

#[instruction]
impl<Reg, Cfg> InstructionIsa<Cfg> for TestMiscInstruction<Reg>
where
    Reg: Register<Type = u64>,
{
    const OWN_ISA_EXTENSIONS: &'static [IsaExtension] = &[];
}

#[instruction]
impl<Reg> fmt::Display for TestMiscInstruction<Reg>
where
    Reg: Register,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {}
    }
}

#[instruction(
    inherit = [Rv64Instruction, ZveXxInstruction, ZvbbInstruction, ZvbcInstruction],
)]
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
enum TestVectorInstruction<Reg> {}

#[instruction]
const impl<Reg> Instruction for TestVectorInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    const ALIGNMENT: u8 = align_of::<u16>() as u8;

    type Reg = Reg;

    #[inline(always)]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

#[instruction]
impl<Reg, Cfg> InstructionIsa<Cfg> for TestVectorInstruction<Reg>
where
    Reg: Register<Type = u64>,
{
    const OWN_ISA_EXTENSIONS: &'static [IsaExtension] = &[];
}

#[instruction]
impl<Reg> fmt::Display for TestVectorInstruction<Reg>
where
    Reg: Register,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {}
    }
}

#[instruction(
    ignore = [Ecall, Rv64ZbsInstruction],
    inherit = [Rv64Instruction, Rv64ZabhaInstruction, Rv64BInstruction],
)]
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
enum TestIgnoreInstruction<Reg> {}

#[instruction]
const impl<Reg> Instruction for TestIgnoreInstruction<Reg>
where
    Reg: [const] Register<Type = u64>,
{
    const ALIGNMENT: u8 = align_of::<u16>() as u8;

    type Reg = Reg;

    #[inline(always)]
    fn try_decode(instruction: u32) -> Option<Self> {
        None
    }

    #[inline(always)]
    fn size(&self) -> u8 {
        size_of::<u32>() as u8
    }
}

#[instruction]
impl<Reg, Cfg> InstructionIsa<Cfg> for TestIgnoreInstruction<Reg>
where
    Reg: Register<Type = u64>,
{
    const OWN_ISA_EXTENSIONS: &'static [IsaExtension] = &[];
}

#[instruction]
impl<Reg> fmt::Display for TestIgnoreInstruction<Reg>
where
    Reg: Register,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {}
    }
}

fn names(extension_lists: &[&[IsaExtension]]) -> Vec<&'static str> {
    let (extensions, len) = IsaExtension::canonical_set(extension_lists);
    extensions[..len]
        .iter()
        .map(|extension| extension.name)
        .collect()
}

#[test]
fn canonical_order() {
    let extensions = [
        "zvl32b", "zbb", "sstc", "c", "zicsr", "zvl128b", "xcustom", "m", "zmmul", "zba", "a",
        "zifencei", "i", "zca", "v", "zve32x",
    ]
    .map(|name| IsaExtension::new(name, 1, 0));

    assert_eq!(
        names(&[&extensions]),
        [
            "i", "m", "a", "c", "v", "zicsr", "zifencei", "zmmul", "zca", "zba", "zbb", "zve32x",
            "zvl128b", "zvl32b", "sstc", "xcustom",
        ]
    );
}

#[test]
fn canonical_set_deduplicates() {
    let first = [
        IsaExtension::new("zicsr", 2, 0),
        IsaExtension::new("i", 2, 1),
    ];
    let second = [
        IsaExtension::new("i", 2, 1),
        IsaExtension::new("zicsr", 2, 0),
        IsaExtension::new("m", 2, 0),
    ];

    assert_eq!(names(&[&first, &second, &[]]), ["i", "m", "zicsr"]);
}

#[test]
fn isa_string() {
    let extensions = [
        IsaExtension::new("i", 2, 1),
        IsaExtension::new("m", 2, 0),
        IsaExtension::new("zmmul", 1, 0),
        IsaExtension::new("xcustom", 123, 45),
    ];

    let (isa_string, len) = IsaExtension::isa_string(64, &extensions);
    assert_eq!(
        str::from_utf8(&isa_string[..len]).unwrap(),
        "rv64i2p1_m2p0_zmmul1p0_xcustom123p45"
    );

    let (isa_string, len) = IsaExtension::isa_string(32, &[]);
    assert_eq!(str::from_utf8(&isa_string[..len]).unwrap(), "rv32");
}

struct TestVectorLengths<const ELEN: Elen, const VLEN: Vlen>;

impl<const ELEN: Elen, const VLEN: Vlen> VectorLengths for TestVectorLengths<ELEN, VLEN> {
    const ELEN: Elen = ELEN;
    const VLEN: Vlen = VLEN;
}

// Expected ISA strings below are produced by LLVM for the same set of extensions

#[test]
fn base_isa_string() {
    assert_eq!(
        <Rv32Instruction<Reg<u32>> as InstructionIsa<()>>::ISA_STRING,
        "rv32i2p1"
    );
    assert_eq!(
        <Rv32Instruction<EReg<u32>> as InstructionIsa<()>>::ISA_STRING,
        "rv32e2p0"
    );
    assert_eq!(
        <Rv64Instruction<Reg<u64>> as InstructionIsa<()>>::ISA_STRING,
        "rv64i2p1"
    );
    assert_eq!(
        <Rv64Instruction<EReg<u64>> as InstructionIsa<()>>::ISA_STRING,
        "rv64e2p0"
    );
}

#[test]
fn implied_extensions() {
    // `M` inherits `Zmmul`
    assert_eq!(
        <TestMInstruction<Reg<u64>> as InstructionIsa<()>>::ISA_STRING,
        "rv64i2p1_m2p0_zmmul1p0"
    );
    assert_eq!(
        <TestScalarInstruction<Reg<u64>> as InstructionIsa<()>>::ISA_STRING,
        "rv64i2p1_m2p0_a2p1_c2p0_b1p0_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0_zca1p0_\
        zcb1p0_zcmp1p0_zba1p0_zbb1p0_zbs1p0"
    );
    // `Zkr` inherits `Zicsr` for `seed` CSR access (LLVM needs `+zicsr` explicitly for this)
    assert_eq!(
        <TestMiscInstruction<Reg<u64>> as InstructionIsa<()>>::ISA_STRING,
        "rv64i2p1_zicond1p0_zicsr2p0_zaamo1p0_zabha1p0_zacas1p0_zalasr1p0_zawrs1p0_zbkb1p0_zbkc1p0_\
        zbkx1p0_zkn1p0_zknd1p0_zkne1p0_zknh1p0_zkr1p0"
    );
}

#[test]
fn vector_isa_string() {
    assert_eq!(
        <TestVectorInstruction<Reg<u64>> as InstructionIsa<
            TestVectorLengths<{ Elen::L64 }, { Vlen::L256 }>,
        >>::ISA_STRING,
        "rv64i2p1_zicsr2p0_zvbb1p0_zvbc1p0_zve32x1p0_zve64x1p0_zvkb1p0_zvl128b1p0_zvl256b1p0_\
        zvl32b1p0_zvl64b1p0"
    );
    assert_eq!(
        <TestVectorInstruction<Reg<u64>> as InstructionIsa<
            TestVectorLengths<{ Elen::L32 }, { Vlen::L32 }>,
        >>::ISA_STRING,
        "rv64i2p1_zicsr2p0_zvbb1p0_zvbc1p0_zve32x1p0_zvkb1p0_zvl32b1p0"
    );
}

#[test]
fn ignored_instructions() {
    // Ignoring `ecall` by name doesn't exclude the base ISA, `amocas.b` and `amocas.h` from
    // `Zabha` are missing without `Zacas`, but `Zabha` is still present. Ignoring the whole `Zbs`
    // excludes it together with `B` that inherits it.
    assert_eq!(
        <TestIgnoreInstruction<Reg<u64>> as InstructionIsa<()>>::ISA_STRING,
        "rv64i2p1_zaamo1p0_zabha1p0_zba1p0_zbb1p0"
    );
}
