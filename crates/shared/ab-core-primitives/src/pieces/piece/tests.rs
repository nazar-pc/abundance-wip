use crate::pieces::{Piece, PieceHeader, Record};
use crate::segments::SegmentRoot;
use ab_aligned_buffer::alignment::Align16;
use ab_aligned_buffer::{OwnedAlignedBuffer, SharedAlignedBuffer};
#[cfg(feature = "scale-codec")]
use parity_scale_codec::{Decode, Encode};
#[cfg(feature = "serde")]
use serde::de::Visitor;
#[cfg(feature = "serde")]
use serde::de::value::{BytesDeserializer, Error, SeqDeserializer, StrDeserializer};
#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer};

/// Same as the inner deserializer, but not human-readable like most binary formats
#[cfg(feature = "serde")]
struct BinaryDeserializer<D>(D);

#[cfg(feature = "serde")]
impl<'de, D> Deserializer<'de> for BinaryDeserializer<D>
where
    D: Deserializer<'de, Error = Error>,
{
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.0.deserialize_any(visitor)
    }

    fn is_human_readable(&self) -> bool {
        false
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option
        unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier
        ignored_any
    }
}

#[cfg(feature = "serde")]
#[test]
fn piece_deserialize_binary_checks_length() {
    let bytes = vec![0; Piece::SIZE + 1];

    for length in [0, 5, Piece::SIZE - 1, Piece::SIZE + 1] {
        let deserializer = BinaryDeserializer(BytesDeserializer::new(&bytes[..length]));
        Piece::deserialize(deserializer).unwrap_err();
    }

    let deserializer = BinaryDeserializer(BytesDeserializer::new(&bytes[..Piece::SIZE]));
    let piece = Piece::deserialize(deserializer).unwrap();
    assert_eq!(piece.as_ref().len(), Piece::SIZE);
}

#[cfg(feature = "serde")]
#[test]
// Hex-decoding a full-size piece takes too long under Miri, and there is no `unsafe` code involved
#[cfg_attr(miri, ignore)]
fn piece_deserialize_human_readable_checks_length() {
    let deserializer = StrDeserializer::<Error>::new("0011");
    Piece::deserialize(deserializer).unwrap_err();

    let hex = "00".repeat(Piece::SIZE);
    let deserializer = StrDeserializer::<Error>::new(&hex);
    let piece = Piece::deserialize(deserializer).unwrap();
    assert_eq!(piece.as_ref().len(), Piece::SIZE);
}

#[test]
fn piece_from_parts() {
    let header = PieceHeader {
        segment_root: SegmentRoot::from([1; SegmentRoot::SIZE]),
        ..PieceHeader::default()
    };
    // Only a few chunks, filling all of them would make it too slow under Miri
    let mut record = Record::new_boxed();
    record[0] = [2; _];
    record[Record::NUM_CHUNKS / 2] = [3; _];
    record[Record::NUM_CHUNKS - 1] = [4; _];

    let piece = Piece::from_parts(&header, &record);

    assert_eq!(piece.header, header);
    assert_eq!(piece.record, *record);
}

#[cfg(feature = "serde")]
#[test]
// Deserializing a full-size piece byte by byte takes too long under Miri, and there is no `unsafe`
// code involved
#[cfg_attr(miri, ignore)]
fn piece_deserialize_sequence_checks_length() {
    let bytes = (0..=Piece::SIZE)
        .map(|index| index as u8)
        .collect::<Vec<_>>();

    for length in [0, 5, Piece::SIZE - 1, Piece::SIZE + 1] {
        let deserializer = BinaryDeserializer(SeqDeserializer::<_, Error>::new(
            bytes[..length].iter().copied(),
        ));
        Piece::deserialize(deserializer).unwrap_err();
    }

    let deserializer = BinaryDeserializer(SeqDeserializer::<_, Error>::new(
        bytes[..Piece::SIZE].iter().copied(),
    ));
    let piece = Piece::deserialize(deserializer).unwrap();
    assert_eq!(piece.as_ref(), &bytes[..Piece::SIZE]);
}

#[cfg(feature = "scale-codec")]
#[test]
fn piece_scale_codec() {
    let mut piece = Piece::default();
    piece.as_mut()[0] = 1;
    piece.as_mut()[Piece::SIZE - 1] = 2;

    let encoded = piece.encode();
    assert_eq!(Piece::decode(&mut encoded.as_slice()).unwrap(), piece);
    // Encoded the same way as bytes
    assert_eq!(encoded, piece.as_ref().encode());

    // Incorrect length
    let encoded = vec![0; Piece::SIZE - 1].encode();
    Piece::decode(&mut encoded.as_slice()).unwrap_err();
    let encoded = vec![0; Piece::SIZE + 1].encode();
    Piece::decode(&mut encoded.as_slice()).unwrap_err();
    // Not enough bytes
    let encoded = piece.encode();
    Piece::decode(&mut &encoded[..encoded.len() - 1]).unwrap_err();
}

#[test]
fn piece_from_buffer() {
    let shared = SharedAlignedBuffer::<Align16>::from_bytes(&[1; Piece::SIZE]);
    let ptr = shared.as_byte_ptr();
    // Buffer is used without copying
    let piece = Piece::try_from(shared).unwrap();
    assert_eq!(piece.as_ref().as_ptr(), ptr);
    assert_eq!(piece.buffer().as_byte_ptr(), ptr);
    assert_eq!(piece.as_ref(), [1; Piece::SIZE]);

    let owned = OwnedAlignedBuffer::<Align16>::from_bytes(&[2; Piece::SIZE]);
    let ptr = owned.as_byte_ptr();
    let piece = Piece::try_from(owned).unwrap();
    assert_eq!(piece.as_ref().as_ptr(), ptr);
    assert_eq!(SharedAlignedBuffer::from(piece).as_byte_ptr(), ptr);

    Piece::try_from(SharedAlignedBuffer::<Align16>::from_bytes(
        &[1; Piece::SIZE - 1],
    ))
    .unwrap_err();
    Piece::try_from(OwnedAlignedBuffer::<Align16>::from_bytes(
        &[1; Piece::SIZE + 1],
    ))
    .unwrap_err();
    Piece::try_from(&[1; Piece::SIZE - 1][..]).unwrap_err();
}

#[test]
fn piece_copy_on_write() {
    let mut piece = Piece::default();
    let ptr = piece.as_ref().as_ptr();
    // The only reference is modified in place
    piece.as_mut()[0] = 1;
    assert_eq!(piece.as_ref().as_ptr(), ptr);

    let cloned = piece.clone();
    assert_eq!(cloned.as_ref().as_ptr(), ptr);
    // Shared piece is copied before modification
    piece.header.segment_root = SegmentRoot::from([2; SegmentRoot::SIZE]);
    assert_ne!(piece.as_ref().as_ptr(), ptr);
    assert_eq!(cloned.as_ref().as_ptr(), ptr);
    assert_eq!(piece.as_ref()[0], 1);
    assert_eq!(
        cloned.header.segment_root,
        SegmentRoot::from([0; SegmentRoot::SIZE])
    );
    assert_ne!(piece, cloned);
}
