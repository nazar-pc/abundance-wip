use crate::fixed_capacity_elements::FixedCapacityElements;

/// Container for storing a number of bytes limited by the specified fixed capacity.
///
/// This is [`FixedCapacityElements`] of `u8`, see its documentation for details.
///
/// `CAPACITY` must not exceed `u32::MAX - 4`, such that the size of the container, which includes
/// 4 bytes of the length, is smaller than 2^32.
pub type FixedCapacityBytes<const CAPACITY: usize> = FixedCapacityElements<u8, CAPACITY>;
