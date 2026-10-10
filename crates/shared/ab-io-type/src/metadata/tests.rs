use crate::metadata::IoTypeMetadataKind;

#[test]
fn check_repr() {
    let known_variants = [
        (IoTypeMetadataKind::Unit, 0),
        (IoTypeMetadataKind::Bool, 1),
        (IoTypeMetadataKind::U8, 2),
        (IoTypeMetadataKind::U16, 3),
        (IoTypeMetadataKind::U32, 4),
        (IoTypeMetadataKind::U64, 5),
        (IoTypeMetadataKind::U128, 6),
        (IoTypeMetadataKind::I8, 7),
        (IoTypeMetadataKind::I16, 8),
        (IoTypeMetadataKind::I32, 9),
        (IoTypeMetadataKind::I64, 10),
        (IoTypeMetadataKind::I128, 11),
        (IoTypeMetadataKind::Struct, 12),
        (IoTypeMetadataKind::TupleStruct, 13),
        (IoTypeMetadataKind::Enum, 14),
        (IoTypeMetadataKind::EnumNoFields, 15),
        (IoTypeMetadataKind::Array, 16),
        (IoTypeMetadataKind::VariableBytes, 17),
        (IoTypeMetadataKind::VariableElements, 18),
        (IoTypeMetadataKind::FixedCapacityBytes8b, 19),
        (IoTypeMetadataKind::FixedCapacityBytes16b, 20),
        (IoTypeMetadataKind::FixedCapacityString8b, 21),
        (IoTypeMetadataKind::FixedCapacityString16b, 22),
        (IoTypeMetadataKind::Unaligned, 23),
    ];

    for (kind, repr_byte) in known_variants {
        assert_eq!(kind as u8, repr_byte);
        assert_eq!(IoTypeMetadataKind::try_from(repr_byte), Ok(kind));
    }

    for byte in known_variants.len() as u8..=u8::MAX {
        assert_eq!(IoTypeMetadataKind::try_from(byte), Err(()));
    }
}
