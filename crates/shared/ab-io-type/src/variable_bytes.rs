#[cfg(test)]
mod tests;

use crate::metadata::{IoTypeMetadataKind, MAX_METADATA_CAPACITY, concat_metadata_sources};
use crate::trivial_type::TrivialType;
use crate::{DerefWrapper, IoType, IoTypeOptional};
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use core::{ptr, slice};

/// Container for storing variable number of bytes.
///
/// `RECOMMENDED_ALLOCATION` is what is being used when a host needs to allocate memory for call
/// into guest, but guest may receive an allocation with more or less memory in practice depending
/// on other circumstances, like when called from another contract with specific allocation
/// specified.
#[derive(Debug)]
#[repr(C)]
pub struct VariableBytes<const RECOMMENDED_ALLOCATION: u32 = 0> {
    bytes: NonNull<u8>,
    size: NonNull<u32>,
    capacity: u32,
}

// SAFETY: Low-level (effectively internal) implementation that upholds safety requirements
unsafe impl<const RECOMMENDED_ALLOCATION: u32> IoType for VariableBytes<RECOMMENDED_ALLOCATION> {
    const METADATA: &[u8] = {
        const fn metadata(recommended_allocation: u32) -> ([u8; MAX_METADATA_CAPACITY], usize) {
            concat_metadata_sources(&[
                &[IoTypeMetadataKind::VariableBytes as u8],
                &recommended_allocation.to_le_bytes(),
            ])
        }

        // Strange syntax to allow Rust to extend the lifetime of metadata scratch automatically
        metadata(RECOMMENDED_ALLOCATION)
            .0
            .split_at(metadata(RECOMMENDED_ALLOCATION).1)
            .0
    };

    // TODO: Use `[u8; U32_TO_USIZE::<RECOMMENDED_ALLOCATION>]` with `generic_const_args`
    type PointerType = u8;

    #[inline(always)]
    fn size(&self) -> u32 {
        self.size()
    }

    #[inline(always)]
    fn capacity(&self) -> u32 {
        self.capacity
    }

    #[inline(always)]
    #[track_caller]
    unsafe fn set_size(&mut self, size: u32) {
        debug_assert!(
            size <= self.capacity,
            "`set_size` called with invalid input {size} for capacity {}",
            self.capacity
        );

        // SAFETY: guaranteed to be initialized by constructors
        unsafe {
            self.size.write(size);
        }
    }

    #[inline(always)]
    #[track_caller]
    unsafe fn from_ptr<'a>(
        ptr: &'a NonNull<Self::PointerType>,
        size: &'a u32,
        capacity: u32,
    ) -> impl Deref<Target = Self> + 'a {
        debug_assert!(ptr.is_aligned(), "Misaligned pointer");
        debug_assert!(
            *size <= capacity,
            "Size {size} must not exceed capacity {capacity}"
        );

        DerefWrapper(Self {
            bytes: *ptr,
            size: NonNull::from_ref(size),
            capacity,
        })
    }

    #[inline(always)]
    #[track_caller]
    unsafe fn from_mut_ptr<'a>(
        ptr: &'a mut NonNull<Self::PointerType>,
        size: &'a mut u32,
        capacity: u32,
    ) -> impl DerefMut<Target = Self> + 'a {
        debug_assert!(ptr.is_aligned(), "Misaligned pointer");
        debug_assert!(
            *size <= capacity,
            "Size {size} must not exceed capacity {capacity}"
        );

        DerefWrapper(Self {
            bytes: *ptr,
            size: NonNull::from_mut(size),
            capacity,
        })
    }

    #[inline(always)]
    fn as_ptr(&self) -> NonNull<Self::PointerType> {
        self.bytes
    }

    #[inline(always)]
    fn as_mut_ptr(&mut self) -> NonNull<Self::PointerType> {
        self.bytes
    }
}

// SAFETY: Size `0` means there are no initialized bytes, contents are only accessed up to the size
unsafe impl<const RECOMMENDED_ALLOCATION: u32> IoTypeOptional
    for VariableBytes<RECOMMENDED_ALLOCATION>
{
}

impl<const RECOMMENDED_ALLOCATION: u32> VariableBytes<RECOMMENDED_ALLOCATION> {
    /// Create a new shared instance from provided memory buffer.
    ///
    /// Returns `None` if `buffer.len() != size`.
    //
    // `impl Deref` is used to tie lifetime of returned value to inputs, but still treat it as a
    // shared reference for most practical purposes.
    #[inline(always)]
    pub const fn from_buffer<'a>(
        buffer: &'a [<Self as IoType>::PointerType],
        size: &'a u32,
    ) -> Option<impl Deref<Target = Self> + 'a> {
        if buffer.len() != *size as usize {
            return None;
        }

        Some(DerefWrapper(Self {
            bytes: NonNull::new(buffer.as_ptr().cast_mut()).expect("Not null; qed"),
            size: NonNull::from_ref(size),
            capacity: *size,
        }))
    }

    /// Create a new exclusive instance from provided memory buffer.
    ///
    /// Returns `None` if `buffer.len() != size`.
    //
    // `impl DerefMut` is used to tie lifetime of returned value to inputs, but still treat it as an
    // exclusive reference for most practical purposes.
    #[inline(always)]
    pub fn from_buffer_mut<'a>(
        buffer: &'a mut [<Self as IoType>::PointerType],
        size: &'a mut u32,
    ) -> Option<impl DerefMut<Target = Self> + 'a> {
        if buffer.len() != *size as usize {
            return None;
        }

        Some(DerefWrapper(Self {
            bytes: NonNull::new(buffer.as_mut_ptr()).expect("Not null; qed"),
            size: NonNull::from_mut(size),
            capacity: *size,
        }))
    }

    /// Create a new exclusive instance from provided uninitialized memory buffer.
    ///
    /// `size` must be `0` since none of the bytes are initialized yet, the whole buffer is used as
    /// capacity.
    ///
    /// Returns `None` if `size != 0` or if the buffer is larger than `u32::MAX` bytes.
    //
    // `impl DerefMut` is used to tie lifetime of returned value to inputs, but still treat it as an
    // exclusive reference for most practical purposes.
    #[inline(always)]
    pub fn from_uninit<'a>(
        uninit: &'a mut [MaybeUninit<<Self as IoType>::PointerType>],
        size: &'a mut u32,
    ) -> Option<impl DerefMut<Target = Self> + 'a> {
        if *size != 0 {
            return None;
        }
        let capacity = u32::try_from(uninit.len()).ok()?;

        Some(DerefWrapper(Self {
            bytes: NonNull::new(uninit.as_mut_ptr().cast_init()).expect("Not null; qed"),
            size: NonNull::from_mut(size),
            capacity,
        }))
    }

    // Size in bytes
    #[inline(always)]
    pub const fn size(&self) -> u32 {
        // SAFETY: guaranteed to be initialized by constructors
        unsafe { self.size.read() }
    }

    /// Capacity in bytes
    #[inline(always)]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Try to get access to initialized bytes
    #[inline(always)]
    pub const fn get_initialized(&self) -> &[u8] {
        let size = self.size();
        let ptr = self.bytes.as_ptr();
        // SAFETY: guaranteed by constructor and explicit methods by the user
        unsafe { slice::from_raw_parts(ptr, size as usize) }
    }

    /// Try to get exclusive access to initialized `Data`, returns `None` if not initialized
    #[inline(always)]
    pub fn get_initialized_mut(&mut self) -> &mut [u8] {
        let size = self.size();
        let ptr = self.bytes.as_ptr();
        // SAFETY: guaranteed by constructor and explicit methods by the user
        unsafe { slice::from_raw_parts_mut(ptr, size as usize) }
    }

    /// Append some bytes by using more of allocated, but currently unused bytes.
    ///
    /// `true` is returned on success, but if there isn't enough unused bytes left, `false` is.
    #[inline(always)]
    #[must_use = "Operation may fail"]
    pub fn append(&mut self, bytes: &[u8]) -> bool {
        let size = self.size();
        let Some(new_size) = u32::try_from(bytes.len())
            .ok()
            .and_then(|appended_size| size.checked_add(appended_size))
        else {
            return false;
        };
        if new_size > self.capacity {
            return false;
        }

        // May overflow, which is not allowed
        let Ok(offset) = isize::try_from(size) else {
            return false;
        };

        // SAFETY: allocation range and offset are checked above, the allocation itself is
        // guaranteed by constructors
        let start = unsafe { self.bytes.offset(offset) };
        // SAFETY: Alignment is the same, writing happens in properly allocated memory guaranteed by
        // constructors, number of bytes is checked above, Rust ownership rules will prevent any
        // overlap here (creating reference to non-initialized part of allocation would already be
        // undefined behavior anyway). The new size covers initialized bytes only and is checked to
        // be within capacity above.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), start.as_ptr(), bytes.len());
            self.size.write(new_size);
        }

        true
    }

    /// Truncate internal initialized bytes to this size.
    ///
    /// Returns `true` on success or `false` if `new_size` is larger than [`Self::size()`].
    #[inline(always)]
    #[must_use = "Operation may fail"]
    pub fn truncate(&mut self, new_size: u32) -> bool {
        if new_size > self.size() {
            return false;
        }

        // SAFETY: guaranteed to be initialized by constructors
        unsafe {
            self.size.write(new_size);
        }

        true
    }

    /// Copy contents from another `IoType`.
    ///
    /// Returns `false` if actual capacity of the instance is not enough to copy contents of `src`
    #[inline(always)]
    #[must_use = "Operation may fail"]
    pub fn copy_from<T>(&mut self, src: &T) -> bool
    where
        T: IoType,
    {
        let src_size = src.size();
        if src_size > self.capacity {
            return false;
        }

        // SAFETY: `src` can't be the same as `&mut self` if invariants of constructor arguments
        // were upheld, size is checked to be within capacity above
        unsafe {
            self.bytes
                .copy_from_nonoverlapping(src.as_ptr().cast::<u8>(), src_size as usize);
            self.size.write(src_size);
        }

        true
    }

    /// Get an exclusive raw pointer to the underlying memory.
    ///
    /// Can be used for initialization with [`Self::assume_init()`] called afterward to confirm how
    /// many bytes are in use right now.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> NonNull<u8> {
        self.bytes
    }

    /// Cast a shared reference to this instance into a reference to an instance of a different
    /// recommended allocation
    #[inline(always)]
    pub fn cast_ref<const DIFFERENT_RECOMMENDED_ALLOCATION: u32>(
        &self,
    ) -> &VariableBytes<DIFFERENT_RECOMMENDED_ALLOCATION> {
        // SAFETY: `VariableBytes` has a fixed layout due to `#[repr(C)]`, which doesn't depend on
        // recommended allocation
        unsafe {
            NonNull::from_ref(self)
                .cast::<VariableBytes<DIFFERENT_RECOMMENDED_ALLOCATION>>()
                .as_ref()
        }
    }

    /// Cast an exclusive reference to this instance into a reference to an instance of a different
    /// recommended allocation
    #[inline(always)]
    pub fn cast_mut<const DIFFERENT_RECOMMENDED_ALLOCATION: u32>(
        &mut self,
    ) -> &mut VariableBytes<DIFFERENT_RECOMMENDED_ALLOCATION> {
        // SAFETY: `VariableBytes` has a fixed layout due to `#[repr(C)]`, which doesn't depend on
        // recommended allocation
        unsafe {
            NonNull::from_mut(self)
                .cast::<VariableBytes<DIFFERENT_RECOMMENDED_ALLOCATION>>()
                .as_mut()
        }
    }

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

        let ptr = self.bytes.cast::<T>();

        // SAFETY: Trivial types are safe to read as bytes, pointer validity is a guaranteed
        // internal invariant
        let value = unsafe {
            if ptr.is_aligned() {
                ptr.read()
            } else {
                ptr.read_unaligned()
            }
        };

        Some(value)
    }

    /// Assume that the first `size` are initialized and can be read.
    ///
    /// Returns `Some(initialized_bytes)` on success or `None` if `size` is larger than its
    /// capacity.
    ///
    /// # Safety
    /// Caller must ensure `size` is actually initialized
    #[inline(always)]
    #[must_use = "Operation may fail"]
    pub unsafe fn assume_init(&mut self, size: u32) -> Option<&mut [u8]> {
        if size > self.capacity {
            return None;
        }

        // SAFETY: guaranteed to be initialized by constructors
        unsafe {
            self.size.write(size);
        }
        Some(self.get_initialized_mut())
    }
}
