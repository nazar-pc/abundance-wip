//! Attribute macro implementing "const fn specialization" as described in
//! <https://internals.rust-lang.org/t/const-fn-specialization/24488/11>.
//!
//! The idea: write a `const fn` and a plain `fn` with the same name, signature and visibility,
//! and annotate both with `#[const_fn_specialization]`. The macro renames each of them to a
//! `#[doc(hidden)]` implementation function and generates a `const fn` with the original name
//! that uses [`core::intrinsics::const_eval_select`] to call the `const fn` implementation when
//! evaluated at compile time and the plain `fn` implementation when called at runtime. This is
//! useful when a `const`-compatible implementation is meaningfully slower than what is possible
//! with runtime-only features (SIMD, intrinsics not available in `const` contexts, etc.).
//!
//! This crate itself doesn't need any nightly features (it is a regular, stable
//! `#[proc_macro_attribute]`), but the code it generates calls
//! [`core::intrinsics::const_eval_select`], which is unstable. Because of that, **every crate
//! that uses `#[const_fn_specialization]`** must enable the following features itself (unlike the
//! macro, these can't be hidden inside this crate: feature gates are checked at the site the
//! generated code ends up in, i.e. the caller's crate):
//!
//! ```ignore
//! #![feature(const_eval_select)]
//! #![feature(core_intrinsics)]
//! #![allow(internal_features)]
//! ```
//!
//! # Example
//!
//! ```
//! # #![feature(const_eval_select)]
//! # #![feature(core_intrinsics)]
//! # #![allow(internal_features)]
//! use ab_const_fn_specialization::const_fn_specialization;
//!
//! #[const_fn_specialization]
//! const fn add(a: u32, b: u32) -> u32 {
//!     // Implementation usable in `const` context.
//!     a + b
//! }
//!
//! #[const_fn_specialization]
//! fn add(a: u32, b: u32) -> u32 {
//!     // Implementation that may use anything not available in `const` context, such as SIMD.
//!     a + b
//! }
//!
//! // Evaluated at compile time using the `const fn` implementation above.
//! const SUM: u32 = add(1, 2);
//! assert_eq!(SUM, 3);
//! // Called at runtime using the plain `fn` implementation above.
//! assert_eq!(add(3, 4), 7);
//! ```
//!
//! Generic functions, lifetimes, references, `where` clauses and arbitrary parameter patterns are
//! all supported:
//!
//! ```
//! # #![feature(const_eval_select)]
//! # #![feature(core_intrinsics)]
//! # #![allow(internal_features)]
//! use ab_const_fn_specialization::const_fn_specialization;
//!
//! #[const_fn_specialization]
//! const fn first<T: Copy>(items: &[T]) -> T {
//!     items[0]
//! }
//!
//! #[const_fn_specialization]
//! fn first<T: Copy>(items: &[T]) -> T {
//!     items[0]
//! }
//!
//! const FIRST: u32 = first(&[1, 2, 3]);
//! assert_eq!(FIRST, 1);
//! assert_eq!(first(&["a", "b"]), "a");
//! ```
//!
//! # Limitations
//!
//! * Both annotated items must appear in the same module and share the same name, visibility,
//!   generics, parameter list and return type; the macro has no way to check this beyond what the
//!   compiler itself enforces once both expansions are in scope.
//! * `self`/method receivers are not supported, only free functions.

pub use ab_const_fn_specialization_macro::const_fn_specialization;
