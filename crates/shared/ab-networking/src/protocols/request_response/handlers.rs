//! Handlers for different request-response protocols

pub mod cached_piece_by_index;
pub mod generic_request_handler;
pub mod piece_by_index;
pub mod segment_header;

use crate::protocols::request_response::handlers::generic_request_handler::encode_to_buffer;
use crate::protocols::request_response::request_response_factory::ResponsePayload;
use ab_aligned_buffer::SharedAlignedBuffer;
use ab_aligned_buffer::alignment::Align1;
use ab_core_primitives::pieces::Piece;
use parity_scale_codec::{Compact, Decode, Encode};

/// Append `prefix` and `piece` to the payload, encoded the same way as SCALE codec encodes them,
/// without copying the contents of the piece
fn encode_piece<P>(payload: &mut ResponsePayload, prefix: &P, piece: &Piece) -> Option<()>
where
    P: Encode + ?Sized,
{
    // Piece is encoded as its length followed by its contents
    let length = Compact(u32::try_from(Piece::SIZE).expect("Piece is smaller than 4 GiB; qed"));
    payload.push(encode_to_buffer(&(prefix, length))?);
    payload.push(piece.buffer().clone());

    Some(())
}

/// Decode a piece from `input`, which is the end of `payload`, without copying its contents, see
/// [`encode_piece()`]
fn decode_piece(
    payload: &SharedAlignedBuffer<Align1>,
    input: &mut &[u8],
) -> Result<Piece, parity_scale_codec::Error> {
    let Compact(length) = Compact::<u32>::decode(input)?;
    let offset = payload.as_slice().len() - input.len();

    let piece = u32::try_from(offset)
        .ok()
        .and_then(|offset| payload.get_slice(offset..offset.checked_add(length)?))
        .ok_or("Not enough bytes for a piece")?;
    let piece = Piece::try_from(piece).map_err(|()| "Incorrect piece length")?;
    *input = input
        .get(Piece::SIZE..)
        .ok_or("Not enough bytes for a piece")?;

    Ok(piece)
}
