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
* `IoTypeMetadataKind` no longer has kinds for specific values and widths of numbers, and decoders reject enums without
  fields encoded as `Enum` instead of `EnumNoFields`, so each type has exactly one encoding now and the same type always
  has the same compact metadata, previously decoders accepted several encodings of the same type, like `Array16b` with 5
  elements next to `Array8b`. `Struct0`..`Struct10`, `TupleStruct1`..`TupleStruct10`, `Enum1`..`Enum10`,
  `EnumNoFields1`..`EnumNoFields10`, `ArrayU8x8`..`ArrayU8x4096` and `VariableElements0` are removed, structs and enums
  always encode the number of fields and variants. `Array8b`..`Array32b` and `VariableElements8b`..`VariableElements32b`
  are replaced with `Array` and `VariableElements`, which encode the number of elements or recommended allocation in 4
  bytes. Remaining kinds are renumbered, so `METADATA` of structs, enums, arrays, `VariableElements`, fixed capacity
  bytes and strings and `Unaligned` changes, and so do fingerprints of methods that use them.
  `IoTypeMetadataKind::compact()` turns all structs into tuple structs, previously structs with more than 10 named
  fields kept the `Struct` kind with field names removed, which is not valid metadata and differs from compact metadata
  of a tuple struct with the same fields
* `IoTypeMetadataKind::type_name()` returns `None` for metadata that `IoTypeMetadataKind::type_details()` rejects,
  previously it returned a name without decoding the rest of the metadata
* `VariableBytes` is now an alias of `VariableElements<u8>` instead of a separate type with the same API. Its `METADATA`
  is that of `VariableElements<u8>` (`VariableBytes0`..`VariableBytes1048576` and `VariableBytes8b`..`VariableBytes32b`
  metadata kinds are removed), so fingerprints of methods that use it change. `copy_from()` only copies from an instance
  of the same type, previously it accepted any `IoType`. `Debug` output and compiler diagnostics show
  `VariableElements<u8>`, and implementations of a trait for both `VariableBytes` and `VariableElements` now conflict
* Derived `TrivialType` of structs with braces and no fields (`struct S {}`) uses the `Struct` metadata kind, previously
  it used `TupleStruct`, because named fields were detected by the first field. Compact metadata is the same either
  way, since structs turn into tuple structs in it
* `TrivialType` can no longer be derived for enums with explicit discriminants that are not equal to variant indices,
  metadata identifies variants by their index and doesn't record discriminants, so previously it didn't match values
  of such enums
* `TrivialType` derive checks all `#[repr(..)]` attributes instead of only the first one and requires exactly
  `#[repr(u8)]` for enums, previously `#[repr(C, u8)]` (which has different offsets of fields) was accepted and options
  in other attributes were only rejected by assertions about metadata, if at all

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
* `IoTypeMetadataKind::type_details()` now returns the alignment of elements for `VariableElements<T>` with the default
  recommended allocation of 0, previously it returned 1, so data placed according to metadata, like inputs in
  transaction payloads and buffers for outputs, could be misaligned

# 0.2.0

Breaking changes:

* Migrate from `generic_const_exprs` to `generic_const_args` family of nightly features

# 0.1.0

Initial release
