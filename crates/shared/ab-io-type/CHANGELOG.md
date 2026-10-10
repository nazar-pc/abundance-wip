# 0.2.1

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

# 0.2.0

Breaking changes:

* Migrate from `generic_const_exprs` to `generic_const_args` family of nightly features

# 0.1.0

Initial release
