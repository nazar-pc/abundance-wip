//! Pointers to the contents of `IoType` instances are returned by value. With a reference to the
//! internal pointer, safe code could point an instance to arbitrary memory, which
//! `get_initialized()` and other methods would then read and write.

use ab_io_type::IoType;
use ab_io_type::variable_bytes::VariableBytes;
use ab_io_type::variable_elements::VariableElements;
use core::ptr::NonNull;

fn repoint_bytes(bytes: &mut VariableBytes) {
    *bytes.as_mut_ptr() = NonNull::dangling();
}

fn repoint_elements(elements: &mut VariableElements<u32>) {
    *elements.as_mut_ptr() = NonNull::dangling();
}

fn repoint_io_type<T>(value: &mut T)
where
    T: IoType,
{
    *IoType::as_mut_ptr(value) = NonNull::dangling();
}

fn main() {}
