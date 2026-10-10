# 0.3.0

Breaking changes:

* `IoType::as_ptr()` and `IoType::as_mut_ptr()` are now safe and return `NonNull` by value instead of a reference to
  the internal pointer, `VariableBytes::as_mut_ptr()` and `VariableElements::as_mut_ptr()` also return `NonNull` now,
  previously they returned `&mut NonNull`, which allowed safe code to point an instance to arbitrary memory
* `from_buffer()` and `from_buffer_mut()` of `VariableBytes` and `VariableElements` now return `None` if the size
  doesn't match the buffer, same for `MaybeData::from_mut()` with a size other than `0` or the size of data, previously
  this was only checked in debug builds, and memory beyond the buffer was exposed otherwise
* `from_uninit()` of `VariableBytes`, `VariableElements` and `MaybeData` now returns `None` if the size is not `0` or if
  the buffer is larger than `u32::MAX` bytes, previously a non-zero size exposed uninitialized memory as initialized
  (`VariableBytes` and `VariableElements` accepted it even in debug builds) and capacity of a larger buffer was
  truncated
* `IoTypeOptional` is now an `unsafe` trait, previously it could be implemented for a type without a valid empty state,
  like any `TrivialType`, which gave `#[contract]` methods access to uninitialized memory of empty `#[slot]` and
  `#[tmp]` storage
* `FixedCapacityBytesU8::copy_from()` and `FixedCapacityBytesU16::copy_from()` no longer have an unused type parameter,
  previously they couldn't be called without specifying it
* Derived `TrivialType` of structs with braces and no fields (`struct S {}`) uses the `Struct0` metadata kind,
  previously it used `TupleStruct` with zero fields, because named fields were detected by the first field.
  `IoTypeMetadataKind::compact()` turns `Struct0` into `TupleStruct` with zero fields, like other structs into tuple
  structs, so compact metadata of such structs and fingerprints of methods that use them don't change
* `ArrayU8x2028` and `VariableBytes2028` metadata kinds are renamed to `ArrayU8x2048` and `VariableBytes2048` and are
  used for `[u8; 2048]` and `VariableBytes<2048>`, previously 2028 was a typo and these types used generic kinds

Fixes:

* `FixedCapacityBytesU8`/`FixedCapacityStringU8` with capacity above `u8::MAX` and
  `FixedCapacityBytesU16`/`FixedCapacityStringU16` with capacity above `u16::MAX` or odd capacity no longer compile,
  previously the length silently wrapped or `TrivialType` exposed the padding byte
* `MaybeData` and `VariableElements` with zero-sized types no longer compile, previously `MaybeData` couldn't represent
  absence of data and `VariableElements` panicked
* Arrays of 4 GiB or larger or with `2^32` or more elements can no longer be used as `TrivialType`, previously their
  size and length were truncated
* `FixedCapacityBytesU8::append()` and `FixedCapacityBytesU16::append()` now append bytes after existing contents,
  previously they overwrote the beginning of contents without changing the length
* `VariableElements::copy_from()` no longer reads and writes out of bounds, previously it used the size in bytes as the
  number of elements to copy
* `VariableElements::count()` now returns the number of elements, previously it returned the size in bytes
* `VariableElements::from_uninit()` now sets capacity in bytes, previously it used the number of elements, so only a
  part of the buffer could be used
* `VariableBytes::append()` and `VariableElements::append()` now update the size, previously appended contents were
  ignored and were written through a reference to a single byte or element, which is undefined behavior
* `MaybeData::get_mut_or_init_with()` now copies the value if the initialization function returns a reference to other
  data, previously uninitialized memory was then exposed as initialized
* Enums with 11 to 16 variants can now derive `TrivialType`, previously the derive used metadata kinds that don't
  exist for them and didn't compile
* Derived `TrivialType` of enums that have both variants with and without fields (possible with zero-sized fields) now
  has well-formed metadata with the number of fields of every variant, previously the metadata was malformed when the
  first variant had no fields

# 0.2.0

Breaking changes:

* Migrate from `generic_const_exprs` to `generic_const_args` family of nightly features

# 0.1.0

Initial release
