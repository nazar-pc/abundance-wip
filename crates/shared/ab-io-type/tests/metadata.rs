//! Tests of `METADATA` of built-in types and of types with derived `TrivialType`.
//!
//! Every case checks exact metadata bytes, and that the metadata decodes back into the name, size
//! and alignment of the type, compacts into expected bytes, and that no truncated metadata decodes.

#![expect(dead_code, reason = "Types are only defined for their metadata")]

use ab_io_type::bool::Bool;
use ab_io_type::fixed_capacity_bytes::{FixedCapacityBytesU8, FixedCapacityBytesU16};
use ab_io_type::fixed_capacity_string::{FixedCapacityStringU8, FixedCapacityStringU16};
use ab_io_type::maybe_data::MaybeData;
use ab_io_type::metadata::{IoTypeMetadataKind as Kind, MAX_METADATA_CAPACITY};
use ab_io_type::trivial_type::TrivialType;
use ab_io_type::unaligned::Unaligned;
use ab_io_type::variable_bytes::VariableBytes;
use ab_io_type::variable_elements::VariableElements;

/// Field names used by [`u8_struct()`]
const FIELD_NAMES: [&str; 12] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
/// Variant names used by [`u8_enum()`]
const VARIANT_NAMES: [&str; 17] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
];

/// Declares a struct with named `u8` fields
macro_rules! named_struct {
    ($name:ident, [$( $field:ident ),*]) => {
        #[derive(Copy, Clone, TrivialType)]
        #[repr(C)]
        struct $name {
            $( $field: u8, )*
        }
    };
}

/// Declares a tuple struct with `u8` fields, one for each listed identifier
macro_rules! tuple_struct {
    ($name:ident, [$( $field:ident ),*]) => {
        #[derive(Copy, Clone, TrivialType)]
        #[repr(C)]
        struct $name( $( tuple_struct!(@u8 $field), )* );
    };
    (@u8 $field:ident) => {
        u8
    };
}

/// Declares an enum with variants without fields
macro_rules! fieldless_enum {
    ($name:ident, [$( $variant:ident ),*]) => {
        #[derive(Copy, Clone, TrivialType)]
        #[repr(u8)]
        enum $name {
            $( $variant, )*
        }
    };
}

/// Declares an enum with variants that have a single `value: u8` field
macro_rules! data_enum {
    ($name:ident, [$( $variant:ident ),*]) => {
        #[derive(Copy, Clone, TrivialType)]
        #[repr(u8)]
        enum $name {
            $( $variant { value: u8 }, )*
        }
    };
}

named_struct!(Point, [x, y]);

/// Expected metadata bytes, built piece by piece
#[derive(Debug)]
struct Expected(Vec<u8>);

impl Expected {
    /// Start with a metadata kind
    fn new(kind: Kind) -> Self {
        Self(vec![kind as u8])
    }

    /// Append a metadata kind
    fn kind(mut self, kind: Kind) -> Self {
        self.0.push(kind as u8);
        self
    }

    /// Append a name prefixed with its length
    fn name(mut self, name: &str) -> Self {
        self.0.push(u8::try_from(name.len()).unwrap());
        self.0.extend_from_slice(name.as_bytes());
        self
    }

    /// Append a single byte, like the number of fields or variants
    fn byte(mut self, byte: u8) -> Self {
        self.0.push(byte);
        self
    }

    /// Append a little-endian `u16`
    fn u16(mut self, value: u16) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Append a little-endian `u32`
    fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Append metadata of another type
    fn nested(mut self, nested: &Self) -> Self {
        self.0.extend_from_slice(&nested.0);
        self
    }
}

/// Compact metadata of a single type
fn compact(metadata: &[u8]) -> Vec<u8> {
    let mut compact = [0; MAX_METADATA_CAPACITY];
    let (remainder, compact_remainder) = Kind::compact(metadata, &mut compact).unwrap();
    assert!(remainder.is_empty());
    let compact_length = MAX_METADATA_CAPACITY - compact_remainder.len();

    compact[..compact_length].to_vec()
}

/// Check that metadata decodes into the expected name, capacity and alignment, compacts into the
/// expected bytes, and that no truncated metadata decodes
#[track_caller]
fn check_decoding(
    metadata: &[u8],
    name: &str,
    capacity: usize,
    alignment: usize,
    expected_compact: &Expected,
) {
    assert_eq!(Kind::type_name(metadata), Some(name.as_bytes()));

    let (type_details, remainder) = Kind::type_details(metadata).unwrap();
    assert!(remainder.is_empty());
    assert_eq!(
        usize::try_from(type_details.recommended_capacity).unwrap(),
        capacity
    );
    assert_eq!(usize::from(type_details.alignment.get()), alignment);

    assert_eq!(compact(metadata), expected_compact.0);

    let mut compact_buffer = [0; MAX_METADATA_CAPACITY];
    for length in 0..metadata.len() {
        let truncated = &metadata[..length];

        assert!(Kind::type_details(truncated).is_none());
        assert!(Kind::compact(truncated, &mut compact_buffer).is_none());
    }
}

/// Check `METADATA` of a [`TrivialType`] and its decoding, see [`check_decoding()`]
#[track_caller]
fn check_trivial_type<T>(name: &str, expected: &Expected, expected_compact: &Expected)
where
    T: TrivialType,
{
    assert_eq!(<T as TrivialType>::METADATA, expected.0.as_slice());
    assert_eq!(<T as ab_io_type::IoType>::METADATA, expected.0.as_slice());
    check_decoding(
        T::METADATA,
        name,
        size_of::<T>(),
        align_of::<T>(),
        expected_compact,
    );
}

/// Check `METADATA` of an [`IoType`](ab_io_type::IoType) and its decoding, see
/// [`check_decoding()`]
#[track_caller]
fn check_io_type<T>(
    name: &str,
    capacity: usize,
    alignment: usize,
    expected: &Expected,
    expected_compact: &Expected,
) where
    T: ab_io_type::IoType,
{
    assert_eq!(T::METADATA, expected.0.as_slice());
    check_decoding(T::METADATA, name, capacity, alignment, expected_compact);
}

/// Expected metadata and compact metadata of [`Point`]
fn point() -> (Expected, Expected) {
    let expected = Expected::new(Kind::Struct2)
        .name("Point")
        .name("x")
        .kind(Kind::U8)
        .name("y")
        .kind(Kind::U8);
    let expected_compact = Expected::new(Kind::TupleStruct2)
        .name("")
        .kind(Kind::U8)
        .kind(Kind::U8);

    (expected, expected_compact)
}

/// Expected metadata and compact metadata of a struct declared with [`named_struct!`] or
/// [`tuple_struct!`] using the first `field_count` of [`FIELD_NAMES`]
fn u8_struct(
    kind: Kind,
    compact_kind: Kind,
    name: &str,
    field_count: u8,
    named: bool,
) -> (Expected, Expected) {
    let mut expected = Expected::new(kind).name(name);
    let mut expected_compact = Expected::new(compact_kind).name("");
    if matches!(kind, Kind::Struct | Kind::TupleStruct) {
        expected = expected.byte(field_count);
        expected_compact = expected_compact.byte(field_count);
    }

    for field_name in &FIELD_NAMES[..usize::from(field_count)] {
        if named {
            expected = expected.name(field_name);
        }
        expected = expected.kind(Kind::U8);
        expected_compact = expected_compact.kind(Kind::U8);
    }

    (expected, expected_compact)
}

/// Expected metadata and compact metadata of an enum declared with [`fieldless_enum!`] or
/// [`data_enum!`] using the first `variant_count` of [`VARIANT_NAMES`]
fn u8_enum(kind: Kind, name: &str, variant_count: u8, with_fields: bool) -> (Expected, Expected) {
    let mut expected = Expected::new(kind).name(name);
    let mut expected_compact = Expected::new(kind).name("");
    if matches!(kind, Kind::Enum | Kind::EnumNoFields) {
        expected = expected.byte(variant_count);
        expected_compact = expected_compact.byte(variant_count);
    }

    for variant_name in &VARIANT_NAMES[..usize::from(variant_count)] {
        expected = expected.name(variant_name);
        expected_compact = expected_compact.name("");
        if with_fields {
            expected = expected.byte(1).name("value").kind(Kind::U8);
            expected_compact = expected_compact.byte(1).kind(Kind::U8);
        }
    }

    (expected, expected_compact)
}

/// Check `METADATA` of a struct declared with [`named_struct!`], see [`u8_struct()`]
#[track_caller]
fn check_named_struct<T>(name: &str, field_count: u8, kind: Kind, compact_kind: Kind)
where
    T: TrivialType,
{
    let (expected, expected_compact) = u8_struct(kind, compact_kind, name, field_count, true);
    check_trivial_type::<T>(name, &expected, &expected_compact);
}

/// Check `METADATA` of a struct declared with [`tuple_struct!`], see [`u8_struct()`]
#[track_caller]
fn check_tuple_struct<T>(name: &str, field_count: u8, kind: Kind)
where
    T: TrivialType,
{
    let (expected, expected_compact) = u8_struct(kind, kind, name, field_count, false);
    check_trivial_type::<T>(name, &expected, &expected_compact);
}

/// Check `METADATA` of an enum declared with [`fieldless_enum!`], see [`u8_enum()`]
#[track_caller]
fn check_fieldless_enum<T>(name: &str, variant_count: u8, kind: Kind)
where
    T: TrivialType,
{
    let (expected, expected_compact) = u8_enum(kind, name, variant_count, false);
    check_trivial_type::<T>(name, &expected, &expected_compact);
}

/// Check `METADATA` of an enum declared with [`data_enum!`], see [`u8_enum()`]
#[track_caller]
fn check_data_enum<T>(name: &str, variant_count: u8, kind: Kind)
where
    T: TrivialType,
{
    let (expected, expected_compact) = u8_enum(kind, name, variant_count, true);
    check_trivial_type::<T>(name, &expected, &expected_compact);
}

#[test]
fn primitives() {
    let cases = [
        (<()>::METADATA, Kind::Unit, "()", 0, 1),
        (Bool::METADATA, Kind::Bool, "bool", 1, 1),
        (u8::METADATA, Kind::U8, "u8", 1, 1),
        (u16::METADATA, Kind::U16, "u16", 2, 2),
        (u32::METADATA, Kind::U32, "u32", 4, 4),
        (u64::METADATA, Kind::U64, "u64", 8, 8),
        (u128::METADATA, Kind::U128, "u128", 16, 16),
        (i8::METADATA, Kind::I8, "i8", 1, 1),
        (i16::METADATA, Kind::I16, "i16", 2, 2),
        (i32::METADATA, Kind::I32, "i32", 4, 4),
        (i64::METADATA, Kind::I64, "i64", 8, 8),
        (i128::METADATA, Kind::I128, "i128", 16, 16),
    ];

    for (metadata, kind, name, size, alignment) in cases {
        let expected = Expected::new(kind);

        assert_eq!(metadata, expected.0.as_slice());
        check_decoding(metadata, name, size, alignment, &expected);
    }
}

#[test]
fn arrays_of_u8() {
    // Sizes with a dedicated metadata kind
    let expected = Expected::new(Kind::ArrayU8x8);
    check_trivial_type::<[u8; 8]>("[u8; 8]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x16);
    check_trivial_type::<[u8; 16]>("[u8; 16]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x32);
    check_trivial_type::<[u8; 32]>("[u8; 32]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x64);
    check_trivial_type::<[u8; 64]>("[u8; 64]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x128);
    check_trivial_type::<[u8; 128]>("[u8; 128]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x256);
    check_trivial_type::<[u8; 256]>("[u8; 256]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x512);
    check_trivial_type::<[u8; 512]>("[u8; 512]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x1024);
    check_trivial_type::<[u8; 1024]>("[u8; 1024]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x2048);
    check_trivial_type::<[u8; 2048]>("[u8; 2048]", &expected, &expected);
    let expected = Expected::new(Kind::ArrayU8x4096);
    check_trivial_type::<[u8; 4096]>("[u8; 4096]", &expected, &expected);

    // Other sizes, with the number of elements in 1, 2 or 4 bytes
    let expected = Expected::new(Kind::Array8b).byte(0).kind(Kind::U8);
    check_trivial_type::<[u8; 0]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array8b).byte(1).kind(Kind::U8);
    check_trivial_type::<[u8; 1]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array8b).byte(7).kind(Kind::U8);
    check_trivial_type::<[u8; 7]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array8b).byte(255).kind(Kind::U8);
    check_trivial_type::<[u8; 255]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array16b).u16(257).kind(Kind::U8);
    check_trivial_type::<[u8; 257]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array16b).u16(2028).kind(Kind::U8);
    check_trivial_type::<[u8; 2028]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array16b).u16(65_535).kind(Kind::U8);
    check_trivial_type::<[u8; 65_535]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array32b).u32(65_536).kind(Kind::U8);
    check_trivial_type::<[u8; 65_536]>("[T; N]", &expected, &expected);
    let expected = Expected::new(Kind::Array32b).u32(70_000).kind(Kind::U8);
    check_trivial_type::<[u8; 70_000]>("[T; N]", &expected, &expected);
}

#[test]
fn arrays_of_other_types() {
    let expected = Expected::new(Kind::Array8b).byte(8).kind(Kind::U16);
    check_trivial_type::<[u16; 8]>("[T; N]", &expected, &expected);

    let expected = Expected::new(Kind::Array16b).u16(300).kind(Kind::U64);
    check_trivial_type::<[u64; 300]>("[T; N]", &expected, &expected);

    let expected = Expected::new(Kind::Array8b).byte(2).kind(Kind::ArrayU8x8);
    check_trivial_type::<[[u8; 8]; 2]>("[T; N]", &expected, &expected);

    let expected = Expected::new(Kind::Array8b)
        .byte(3)
        .kind(Kind::Unaligned)
        .kind(Kind::U32);
    check_trivial_type::<[Unaligned<u32>; 3]>("[T; N]", &expected, &expected);

    let (point, point_compact) = point();
    let expected = Expected::new(Kind::Array8b).byte(4).nested(&point);
    let expected_compact = Expected::new(Kind::Array8b).byte(4).nested(&point_compact);
    check_trivial_type::<[Point; 4]>("[T; N]", &expected, &expected_compact);
}

#[test]
fn unaligned() {
    let expected = Expected::new(Kind::Unaligned).kind(Kind::U64);
    check_trivial_type::<Unaligned<u64>>("Unaligned", &expected, &expected);

    let expected = Expected::new(Kind::Unaligned).kind(Kind::ArrayU8x8);
    check_trivial_type::<Unaligned<[u8; 8]>>("Unaligned", &expected, &expected);
}

#[test]
fn fixed_capacity_bytes_and_strings() {
    let expected = Expected::new(Kind::FixedCapacityBytes8b).byte(10);
    check_trivial_type::<FixedCapacityBytesU8<10>>("FixedCapacityBytes", &expected, &expected);

    let expected = Expected::new(Kind::FixedCapacityBytes16b).u16(300);
    check_trivial_type::<FixedCapacityBytesU16<300>>("FixedCapacityBytes", &expected, &expected);

    let expected = Expected::new(Kind::FixedCapacityString8b).byte(10);
    check_trivial_type::<FixedCapacityStringU8<10>>("FixedCapacityString", &expected, &expected);

    let expected = Expected::new(Kind::FixedCapacityString16b).u16(300);
    check_trivial_type::<FixedCapacityStringU16<300>>("FixedCapacityString", &expected, &expected);
}

#[test]
fn variable_bytes() {
    let name = "VariableBytes";

    // Recommended allocations with a dedicated metadata kind
    let expected = Expected::new(Kind::VariableBytes0);
    check_io_type::<VariableBytes<0>>(name, 0, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes512);
    check_io_type::<VariableBytes<512>>(name, 512, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes1024);
    check_io_type::<VariableBytes<1024>>(name, 1024, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes2048);
    check_io_type::<VariableBytes<2048>>(name, 2048, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes4096);
    check_io_type::<VariableBytes<4096>>(name, 4096, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes8192);
    check_io_type::<VariableBytes<8192>>(name, 8192, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes16384);
    check_io_type::<VariableBytes<16_384>>(name, 16_384, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes32768);
    check_io_type::<VariableBytes<32_768>>(name, 32_768, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes65536);
    check_io_type::<VariableBytes<65_536>>(name, 65_536, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes131072);
    check_io_type::<VariableBytes<131_072>>(name, 131_072, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes262144);
    check_io_type::<VariableBytes<262_144>>(name, 262_144, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes524288);
    check_io_type::<VariableBytes<524_288>>(name, 524_288, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes1048576);
    check_io_type::<VariableBytes<1_048_576>>(name, 1_048_576, 1, &expected, &expected);

    // Other recommended allocations, encoded in 1, 2 or 4 bytes
    let expected = Expected::new(Kind::VariableBytes8b).byte(1);
    check_io_type::<VariableBytes<1>>(name, 1, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes8b).byte(255);
    check_io_type::<VariableBytes<255>>(name, 255, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes16b).u16(256);
    check_io_type::<VariableBytes<256>>(name, 256, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes16b).u16(2028);
    check_io_type::<VariableBytes<2028>>(name, 2028, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes16b).u16(65_535);
    check_io_type::<VariableBytes<65_535>>(name, 65_535, 1, &expected, &expected);
    let expected = Expected::new(Kind::VariableBytes32b).u32(65_537);
    check_io_type::<VariableBytes<65_537>>(name, 65_537, 1, &expected, &expected);
}

#[test]
fn variable_elements() {
    let name = "VariableElements";

    let expected = Expected::new(Kind::VariableElements0).kind(Kind::U8);
    check_io_type::<VariableElements<u8>>(name, 0, 1, &expected, &expected);

    // Alignment of elements, regardless of the recommended allocation
    let expected = Expected::new(Kind::VariableElements0).kind(Kind::U128);
    check_io_type::<VariableElements<u128>>(name, 0, 16, &expected, &expected);
    let expected = Expected::new(Kind::VariableElements8b)
        .byte(1)
        .kind(Kind::U128);
    check_io_type::<VariableElements<u128, 1>>(name, 16, 16, &expected, &expected);

    let (point, point_compact) = point();
    let expected = Expected::new(Kind::VariableElements8b)
        .byte(10)
        .nested(&point);
    let expected_compact = Expected::new(Kind::VariableElements8b)
        .byte(10)
        .nested(&point_compact);
    check_io_type::<VariableElements<Point, 10>>(name, 20, 1, &expected, &expected_compact);

    let expected = Expected::new(Kind::VariableElements16b)
        .u16(300)
        .kind(Kind::U32);
    check_io_type::<VariableElements<u32, 300>>(name, 1200, 4, &expected, &expected);

    let expected = Expected::new(Kind::VariableElements32b)
        .u32(70_000)
        .kind(Kind::U16);
    check_io_type::<VariableElements<u16, 70_000>>(name, 140_000, 2, &expected, &expected);
}

#[test]
fn maybe_data() {
    // `MaybeData` has the same metadata as the type inside
    let (point, point_compact) = point();
    check_io_type::<MaybeData<Point>>("Point", 2, 1, &point, &point_compact);

    let expected = Expected::new(Kind::U64);
    check_io_type::<MaybeData<u64>>("u64", 8, 8, &expected, &expected);
}

#[test]
fn empty_structs() {
    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    #[expect(clippy::empty_structs_with_brackets, reason = "Tested")]
    struct Empty {}

    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    #[expect(clippy::empty_structs_with_brackets, reason = "Tested")]
    struct EmptyTuple();

    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    struct EmptyUnit;

    // Compact metadata is the same for all empty structs
    let expected = Expected::new(Kind::Struct0).name("Empty");
    let expected_compact = Expected::new(Kind::TupleStruct).name("").byte(0);
    check_trivial_type::<Empty>("Empty", &expected, &expected_compact);

    let expected = Expected::new(Kind::TupleStruct).name("EmptyTuple").byte(0);
    let expected_compact = Expected::new(Kind::TupleStruct).name("").byte(0);
    check_trivial_type::<EmptyTuple>("EmptyTuple", &expected, &expected_compact);

    let expected = Expected::new(Kind::TupleStruct).name("EmptyUnit").byte(0);
    let expected_compact = Expected::new(Kind::TupleStruct).name("").byte(0);
    check_trivial_type::<EmptyUnit>("EmptyUnit", &expected, &expected_compact);
}

#[test]
fn structs_with_named_fields() {
    named_struct!(Named1, [a]);
    named_struct!(Named2, [a, b]);
    named_struct!(Named3, [a, b, c]);
    named_struct!(Named4, [a, b, c, d]);
    named_struct!(Named5, [a, b, c, d, e]);
    named_struct!(Named6, [a, b, c, d, e, f]);
    named_struct!(Named7, [a, b, c, d, e, f, g]);
    named_struct!(Named8, [a, b, c, d, e, f, g, h]);
    named_struct!(Named9, [a, b, c, d, e, f, g, h, i]);
    named_struct!(Named10, [a, b, c, d, e, f, g, h, i, j]);
    named_struct!(Named11, [a, b, c, d, e, f, g, h, i, j, k]);
    named_struct!(Named12, [a, b, c, d, e, f, g, h, i, j, k, l]);

    check_named_struct::<Named1>("Named1", 1, Kind::Struct1, Kind::TupleStruct1);
    check_named_struct::<Named2>("Named2", 2, Kind::Struct2, Kind::TupleStruct2);
    check_named_struct::<Named3>("Named3", 3, Kind::Struct3, Kind::TupleStruct3);
    check_named_struct::<Named4>("Named4", 4, Kind::Struct4, Kind::TupleStruct4);
    check_named_struct::<Named5>("Named5", 5, Kind::Struct5, Kind::TupleStruct5);
    check_named_struct::<Named6>("Named6", 6, Kind::Struct6, Kind::TupleStruct6);
    check_named_struct::<Named7>("Named7", 7, Kind::Struct7, Kind::TupleStruct7);
    check_named_struct::<Named8>("Named8", 8, Kind::Struct8, Kind::TupleStruct8);
    check_named_struct::<Named9>("Named9", 9, Kind::Struct9, Kind::TupleStruct9);
    check_named_struct::<Named10>("Named10", 10, Kind::Struct10, Kind::TupleStruct10);
    // More than 10 fields need an explicit number of fields
    check_named_struct::<Named11>("Named11", 11, Kind::Struct, Kind::TupleStruct);
    check_named_struct::<Named12>("Named12", 12, Kind::Struct, Kind::TupleStruct);
}

#[test]
fn tuple_structs() {
    tuple_struct!(Tuple1, [a]);
    tuple_struct!(Tuple2, [a, b]);
    tuple_struct!(Tuple3, [a, b, c]);
    tuple_struct!(Tuple4, [a, b, c, d]);
    tuple_struct!(Tuple5, [a, b, c, d, e]);
    tuple_struct!(Tuple6, [a, b, c, d, e, f]);
    tuple_struct!(Tuple7, [a, b, c, d, e, f, g]);
    tuple_struct!(Tuple8, [a, b, c, d, e, f, g, h]);
    tuple_struct!(Tuple9, [a, b, c, d, e, f, g, h, i]);
    tuple_struct!(Tuple10, [a, b, c, d, e, f, g, h, i, j]);
    tuple_struct!(Tuple11, [a, b, c, d, e, f, g, h, i, j, k]);
    tuple_struct!(Tuple12, [a, b, c, d, e, f, g, h, i, j, k, l]);

    check_tuple_struct::<Tuple1>("Tuple1", 1, Kind::TupleStruct1);
    check_tuple_struct::<Tuple2>("Tuple2", 2, Kind::TupleStruct2);
    check_tuple_struct::<Tuple3>("Tuple3", 3, Kind::TupleStruct3);
    check_tuple_struct::<Tuple4>("Tuple4", 4, Kind::TupleStruct4);
    check_tuple_struct::<Tuple5>("Tuple5", 5, Kind::TupleStruct5);
    check_tuple_struct::<Tuple6>("Tuple6", 6, Kind::TupleStruct6);
    check_tuple_struct::<Tuple7>("Tuple7", 7, Kind::TupleStruct7);
    check_tuple_struct::<Tuple8>("Tuple8", 8, Kind::TupleStruct8);
    check_tuple_struct::<Tuple9>("Tuple9", 9, Kind::TupleStruct9);
    check_tuple_struct::<Tuple10>("Tuple10", 10, Kind::TupleStruct10);
    // More than 10 fields need an explicit number of fields
    check_tuple_struct::<Tuple11>("Tuple11", 11, Kind::TupleStruct);
    check_tuple_struct::<Tuple12>("Tuple12", 12, Kind::TupleStruct);
}

#[test]
fn named_and_tuple_structs_compact_equally() {
    named_struct!(Named10, [a, b, c, d, e, f, g, h, i, j]);
    named_struct!(Named11, [a, b, c, d, e, f, g, h, i, j, k]);
    tuple_struct!(Tuple10, [a, b, c, d, e, f, g, h, i, j]);
    tuple_struct!(Tuple11, [a, b, c, d, e, f, g, h, i, j, k]);

    // Names are removed, so only the types of fields remain, regardless of their number
    assert_eq!(compact(Named10::METADATA), compact(Tuple10::METADATA));
    assert_eq!(compact(Named11::METADATA), compact(Tuple11::METADATA));
}

#[test]
fn structs_with_different_field_types() {
    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    struct Primitives {
        a: u128,
        b: i128,
        c: u64,
        d: i64,
        e: u32,
        f: i32,
        g: u16,
        h: i16,
        i: u8,
        j: i8,
        k: Bool,
        l: (),
        padding: [u8; 1],
    }

    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    struct Mixed(u64, Unaligned<u32>, [u8; 4]);

    #[derive(Copy, Clone, TrivialType)]
    #[repr(transparent)]
    struct Transparent(u32);

    let expected = Expected::new(Kind::Struct)
        .name("Primitives")
        .byte(13)
        .name("a")
        .kind(Kind::U128)
        .name("b")
        .kind(Kind::I128)
        .name("c")
        .kind(Kind::U64)
        .name("d")
        .kind(Kind::I64)
        .name("e")
        .kind(Kind::U32)
        .name("f")
        .kind(Kind::I32)
        .name("g")
        .kind(Kind::U16)
        .name("h")
        .kind(Kind::I16)
        .name("i")
        .kind(Kind::U8)
        .name("j")
        .kind(Kind::I8)
        .name("k")
        .kind(Kind::Bool)
        .name("l")
        .kind(Kind::Unit)
        .name("padding")
        .kind(Kind::Array8b)
        .byte(1)
        .kind(Kind::U8);
    let expected_compact = Expected::new(Kind::TupleStruct)
        .name("")
        .byte(13)
        .kind(Kind::U128)
        .kind(Kind::I128)
        .kind(Kind::U64)
        .kind(Kind::I64)
        .kind(Kind::U32)
        .kind(Kind::I32)
        .kind(Kind::U16)
        .kind(Kind::I16)
        .kind(Kind::U8)
        .kind(Kind::I8)
        .kind(Kind::Bool)
        .kind(Kind::Unit)
        .kind(Kind::Array8b)
        .byte(1)
        .kind(Kind::U8);
    check_trivial_type::<Primitives>("Primitives", &expected, &expected_compact);

    let expected = Expected::new(Kind::TupleStruct3)
        .name("Mixed")
        .kind(Kind::U64)
        .kind(Kind::Unaligned)
        .kind(Kind::U32)
        .kind(Kind::Array8b)
        .byte(4)
        .kind(Kind::U8);
    let expected_compact = Expected::new(Kind::TupleStruct3)
        .name("")
        .kind(Kind::U64)
        .kind(Kind::Unaligned)
        .kind(Kind::U32)
        .kind(Kind::Array8b)
        .byte(4)
        .kind(Kind::U8);
    check_trivial_type::<Mixed>("Mixed", &expected, &expected_compact);

    let expected = Expected::new(Kind::TupleStruct1)
        .name("Transparent")
        .kind(Kind::U32);
    let expected_compact = Expected::new(Kind::TupleStruct1).name("").kind(Kind::U32);
    check_trivial_type::<Transparent>("Transparent", &expected, &expected_compact);
}

#[test]
fn fieldless_enums() {
    fieldless_enum!(NoFields1, [A]);
    fieldless_enum!(NoFields2, [A, B]);
    fieldless_enum!(NoFields3, [A, B, C]);
    fieldless_enum!(NoFields4, [A, B, C, D]);
    fieldless_enum!(NoFields5, [A, B, C, D, E]);
    fieldless_enum!(NoFields6, [A, B, C, D, E, F]);
    fieldless_enum!(NoFields7, [A, B, C, D, E, F, G]);
    fieldless_enum!(NoFields8, [A, B, C, D, E, F, G, H]);
    fieldless_enum!(NoFields9, [A, B, C, D, E, F, G, H, I]);
    fieldless_enum!(NoFields10, [A, B, C, D, E, F, G, H, I, J]);
    fieldless_enum!(NoFields11, [A, B, C, D, E, F, G, H, I, J, K]);
    fieldless_enum!(NoFields12, [A, B, C, D, E, F, G, H, I, J, K, L]);
    fieldless_enum!(NoFields16, [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P]);
    fieldless_enum!(
        NoFields17,
        [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q]
    );

    check_fieldless_enum::<NoFields1>("NoFields1", 1, Kind::EnumNoFields1);
    check_fieldless_enum::<NoFields2>("NoFields2", 2, Kind::EnumNoFields2);
    check_fieldless_enum::<NoFields3>("NoFields3", 3, Kind::EnumNoFields3);
    check_fieldless_enum::<NoFields4>("NoFields4", 4, Kind::EnumNoFields4);
    check_fieldless_enum::<NoFields5>("NoFields5", 5, Kind::EnumNoFields5);
    check_fieldless_enum::<NoFields6>("NoFields6", 6, Kind::EnumNoFields6);
    check_fieldless_enum::<NoFields7>("NoFields7", 7, Kind::EnumNoFields7);
    check_fieldless_enum::<NoFields8>("NoFields8", 8, Kind::EnumNoFields8);
    check_fieldless_enum::<NoFields9>("NoFields9", 9, Kind::EnumNoFields9);
    check_fieldless_enum::<NoFields10>("NoFields10", 10, Kind::EnumNoFields10);
    // More than 10 variants need an explicit number of variants
    check_fieldless_enum::<NoFields11>("NoFields11", 11, Kind::EnumNoFields);
    check_fieldless_enum::<NoFields12>("NoFields12", 12, Kind::EnumNoFields);
    check_fieldless_enum::<NoFields16>("NoFields16", 16, Kind::EnumNoFields);
    check_fieldless_enum::<NoFields17>("NoFields17", 17, Kind::EnumNoFields);
}

#[test]
fn enums_with_fields() {
    data_enum!(Data1, [A]);
    data_enum!(Data2, [A, B]);
    data_enum!(Data3, [A, B, C]);
    data_enum!(Data4, [A, B, C, D]);
    data_enum!(Data5, [A, B, C, D, E]);
    data_enum!(Data6, [A, B, C, D, E, F]);
    data_enum!(Data7, [A, B, C, D, E, F, G]);
    data_enum!(Data8, [A, B, C, D, E, F, G, H]);
    data_enum!(Data9, [A, B, C, D, E, F, G, H, I]);
    data_enum!(Data10, [A, B, C, D, E, F, G, H, I, J]);
    data_enum!(Data11, [A, B, C, D, E, F, G, H, I, J, K]);
    data_enum!(Data12, [A, B, C, D, E, F, G, H, I, J, K, L]);
    data_enum!(Data16, [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P]);
    data_enum!(Data17, [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q]);

    check_data_enum::<Data1>("Data1", 1, Kind::Enum1);
    check_data_enum::<Data2>("Data2", 2, Kind::Enum2);
    check_data_enum::<Data3>("Data3", 3, Kind::Enum3);
    check_data_enum::<Data4>("Data4", 4, Kind::Enum4);
    check_data_enum::<Data5>("Data5", 5, Kind::Enum5);
    check_data_enum::<Data6>("Data6", 6, Kind::Enum6);
    check_data_enum::<Data7>("Data7", 7, Kind::Enum7);
    check_data_enum::<Data8>("Data8", 8, Kind::Enum8);
    check_data_enum::<Data9>("Data9", 9, Kind::Enum9);
    check_data_enum::<Data10>("Data10", 10, Kind::Enum10);
    // More than 10 variants need an explicit number of variants
    check_data_enum::<Data11>("Data11", 11, Kind::Enum);
    check_data_enum::<Data12>("Data12", 12, Kind::Enum);
    check_data_enum::<Data16>("Data16", 16, Kind::Enum);
    check_data_enum::<Data17>("Data17", 17, Kind::Enum);
}

#[test]
fn enums_with_different_variants() {
    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum Shape {
        Dot { point: Point },
        Line { from: u8, to: u8 },
        Wide { bytes: [u8; 2] },
    }

    // Variants without fields are only possible next to variants with zero-sized fields, since
    // all variants must have the same size
    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum MixedDataFirst {
        Full { value: () },
        Empty,
    }

    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum MixedFieldlessFirst {
        Empty,
        Full { value: () },
        AlsoEmpty,
        AlsoFull { first: [u8; 0], second: () },
    }

    let (point, point_compact) = point();
    let expected = Expected::new(Kind::Enum3)
        .name("Shape")
        .name("Dot")
        .byte(1)
        .name("point")
        .nested(&point)
        .name("Line")
        .byte(2)
        .name("from")
        .kind(Kind::U8)
        .name("to")
        .kind(Kind::U8)
        .name("Wide")
        .byte(1)
        .name("bytes")
        .kind(Kind::Array8b)
        .byte(2)
        .kind(Kind::U8);
    let expected_compact = Expected::new(Kind::Enum3)
        .name("")
        .name("")
        .byte(1)
        .nested(&point_compact)
        .name("")
        .byte(2)
        .kind(Kind::U8)
        .kind(Kind::U8)
        .name("")
        .byte(1)
        .kind(Kind::Array8b)
        .byte(2)
        .kind(Kind::U8);
    check_trivial_type::<Shape>("Shape", &expected, &expected_compact);

    let expected = Expected::new(Kind::Enum2)
        .name("MixedDataFirst")
        .name("Full")
        .byte(1)
        .name("value")
        .kind(Kind::Unit)
        .name("Empty")
        .byte(0);
    let expected_compact = Expected::new(Kind::Enum2)
        .name("")
        .name("")
        .byte(1)
        .kind(Kind::Unit)
        .name("")
        .byte(0);
    check_trivial_type::<MixedDataFirst>("MixedDataFirst", &expected, &expected_compact);

    let expected = Expected::new(Kind::Enum4)
        .name("MixedFieldlessFirst")
        .name("Empty")
        .byte(0)
        .name("Full")
        .byte(1)
        .name("value")
        .kind(Kind::Unit)
        .name("AlsoEmpty")
        .byte(0)
        .name("AlsoFull")
        .byte(2)
        .name("first")
        .kind(Kind::Array8b)
        .byte(0)
        .kind(Kind::U8)
        .name("second")
        .kind(Kind::Unit);
    let expected_compact = Expected::new(Kind::Enum4)
        .name("")
        .name("")
        .byte(0)
        .name("")
        .byte(1)
        .kind(Kind::Unit)
        .name("")
        .byte(0)
        .name("")
        .byte(2)
        .kind(Kind::Array8b)
        .byte(0)
        .kind(Kind::U8)
        .kind(Kind::Unit);
    check_trivial_type::<MixedFieldlessFirst>("MixedFieldlessFirst", &expected, &expected_compact);
}

#[test]
fn enums_with_explicit_discriminants() {
    const ONE: u8 = 1;

    // Explicit discriminants equal to variant indices don't change metadata, so it is the same as
    // for implicit discriminants in other tests
    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum NoFields3 {
        A = 0,
        B = 1,
        C = 2,
    }

    // Implicit discriminants continue from explicit ones
    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum NoFields12 {
        A,
        B = 1,
        C,
        D,
        E,
        F,
        G,
        H,
        I,
        J,
        K = 10,
        L,
    }

    // Discriminants are constant expressions, which can refer to `Self` and local items
    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum NoFields4 {
        A = Self::FIRST,
        B = ONE,
        C = ONE + 1,
        // The type is inferred from the enum's representation
        D = 3u16 as _,
    }

    impl NoFields4 {
        const FIRST: u8 = 0;
    }

    #[derive(Copy, Clone, TrivialType)]
    #[repr(u8)]
    enum Data2 {
        A { value: u8 } = 0,
        B { value: u8 } = 1,
    }

    check_fieldless_enum::<NoFields3>("NoFields3", 3, Kind::EnumNoFields3);
    check_fieldless_enum::<NoFields12>("NoFields12", 12, Kind::EnumNoFields);
    check_fieldless_enum::<NoFields4>("NoFields4", 4, Kind::EnumNoFields4);
    check_data_enum::<Data2>("Data2", 2, Kind::Enum2);
}

#[test]
fn nested_types() {
    fieldless_enum!(Direction, [A, B]);
    data_enum!(Value, [A, B]);

    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    struct Inner(Point, [u8; 8]);

    #[derive(Copy, Clone, TrivialType)]
    #[repr(C)]
    struct Outer {
        wide: Unaligned<u64>,
        points: [Point; 3],
        direction: Direction,
        value: Value,
        inner: Inner,
    }

    let (point, point_compact) = point();
    let (direction, direction_compact) = u8_enum(Kind::EnumNoFields2, "Direction", 2, false);
    let (value, value_compact) = u8_enum(Kind::Enum2, "Value", 2, true);
    let inner = Expected::new(Kind::TupleStruct2)
        .name("Inner")
        .nested(&point)
        .kind(Kind::ArrayU8x8);
    let inner_compact = Expected::new(Kind::TupleStruct2)
        .name("")
        .nested(&point_compact)
        .kind(Kind::ArrayU8x8);
    check_trivial_type::<Inner>("Inner", &inner, &inner_compact);

    let expected = Expected::new(Kind::Struct5)
        .name("Outer")
        .name("wide")
        .kind(Kind::Unaligned)
        .kind(Kind::U64)
        .name("points")
        .kind(Kind::Array8b)
        .byte(3)
        .nested(&point)
        .name("direction")
        .nested(&direction)
        .name("value")
        .nested(&value)
        .name("inner")
        .nested(&inner);
    let expected_compact = Expected::new(Kind::TupleStruct5)
        .name("")
        .kind(Kind::Unaligned)
        .kind(Kind::U64)
        .kind(Kind::Array8b)
        .byte(3)
        .nested(&point_compact)
        .nested(&direction_compact)
        .nested(&value_compact)
        .nested(&inner_compact);
    check_trivial_type::<Outer>("Outer", &expected, &expected_compact);
}
