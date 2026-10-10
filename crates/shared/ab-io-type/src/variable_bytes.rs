#[cfg(test)]
mod tests;

use crate::trivial_type::TrivialType;
use crate::variable_elements::VariableElements;

/// Container for storing variable number of bytes.
///
/// This is [`VariableElements`] of `u8`, so its recommended allocation in elements is also in
/// bytes.
///
/// `RECOMMENDED_ALLOCATION` is what is being used when a host needs to allocate memory for call
/// into guest, but guest may receive an allocation with more or less memory in practice depending
/// on other circumstances, like when called from another contract with specific allocation
/// specified.
pub type VariableBytes<const RECOMMENDED_ALLOCATION: u32 = 0> =
    VariableElements<u8, RECOMMENDED_ALLOCATION>;

impl<const RECOMMENDED_ALLOCATION: u32> VariableElements<u8, RECOMMENDED_ALLOCATION> {
    /// Reads and returns value of type `T` or `None` if there is not enough data.
    ///
    /// Checks alignment internally to support both aligned and unaligned reads.
    #[inline(always)]
    pub fn read_trivial_type<T>(&self) -> Option<T>
    where
        T: TrivialType,
    {
        if self.size() < T::SIZE {
            return None;
        }

        let ptr = self.get_initialized().as_ptr().cast::<T>();

        // SAFETY: Trivial types are safe to read as bytes, there are at least `T::SIZE` initialized
        // bytes as checked above
        let value = unsafe {
            if ptr.is_aligned() {
                ptr.read()
            } else {
                ptr.read_unaligned()
            }
        };

        Some(value)
    }
}
