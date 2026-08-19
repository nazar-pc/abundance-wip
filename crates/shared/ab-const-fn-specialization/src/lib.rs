#![feature(const_eval_select)]
#![feature(core_intrinsics)]
#![feature(const_trait_impl)]
#![allow(internal_features)]

//! Attribute macro implementing "const fn specialization" as described in
//! <https://internals.rust-lang.org/t/const-fn-specialization/24488/11>.
//!
//! The idea: write a `const fn` and a plain `fn` with the same name, signature and visibility,
//! and annotate both with `#[const_fn_specialization]`. The macro renames each of them to a
//! `#[doc(hidden)]` implementation function and generates a `const fn` with the original name
//! that dispatches to the `const fn` implementation when evaluated at compile time and to the
//! plain `fn` implementation when called at runtime, using
//! [`core::intrinsics::const_eval_select`] under the hood. This is useful when a
//! `const`-compatible implementation is meaningfully slower than what is possible with
//! runtime-only features (SIMD, intrinsics not available in `const` contexts, etc.).
//!
//! Both this crate and the proc-macro crate it depends on build entirely on stable Rust; the only
//! nightly-only bits ([`core::intrinsics::const_eval_select`] and the handful of features needed
//! to call it) are contained in a small set of `#[doc(hidden)]` dispatch helper functions in
//! *this* crate. Crucially, **crates using `#[const_fn_specialization]` don't need to enable any
//! nightly features themselves** — every generated dispatcher just calls one of the helpers below,
//! and unstable-feature checks are resolved against the crate where a function is *defined*, not
//! where it is monomorphized/called from.
//!
//! # Example
//!
//! ```
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
//! * Functions with more than 12 parameters are not supported (matches the arity commonly supported
//!   by the standard library for tuples, e.g. [`Debug`](core::fmt::Debug)).

pub use ab_const_fn_specialization_macro::const_fn_specialization;

/// Generates one `__const_fn_specialization_dispatchN` helper for a given arity.
///
/// Each helper picks `f` at compile time and `g` at runtime. `f` is constrained with `impl const
/// FnOnce(..) -> RET` (rather than a named, bounded generic type parameter) and `g` is a concrete
/// `fn(..) -> RET` pointer (rather than a named generic type parameter) deliberately: at the time
/// of writing, going through a *named* generic type parameter for either of them as the callee of
/// `const_eval_select` triggers an internal compiler error (two different ones, depending on the
/// exact shape), while this shape does not.
macro_rules! impl_dispatch {
    ($name:ident($($arg:ident: $ty:ident),* $(,)?)) => {
        /// # Safety
        /// `f` and `g` must be semantically equivalent implementations of the same function, one
        /// of which happens to be usable in `const` context and the other one isn't. This isn't
        /// enforced by the compiler, only documented (same as for `const_eval_select` itself,
        /// which this function wraps and which is otherwise safe to call).
        #[doc(hidden)]
        #[allow(
            clippy::impl_trait_in_params,
            reason = "Named generic type parameter would trigger the compiler ICE described above"
        )]
        #[allow(clippy::too_many_arguments, reason = "Inherent to generating one function per arity")]
        pub const unsafe fn $name<$($ty,)* RET>(
            $($arg: $ty,)*
            f: impl const FnOnce($($ty),*) -> RET,
            g: fn($($ty),*) -> RET,
        ) -> RET {
            core::intrinsics::const_eval_select(($($arg,)*), f, g)
        }
    };
}

impl_dispatch!(__const_fn_specialization_dispatch0());
impl_dispatch!(__const_fn_specialization_dispatch1(a0: A0));
impl_dispatch!(__const_fn_specialization_dispatch2(a0: A0, a1: A1));
impl_dispatch!(__const_fn_specialization_dispatch3(a0: A0, a1: A1, a2: A2));
impl_dispatch!(__const_fn_specialization_dispatch4(a0: A0, a1: A1, a2: A2, a3: A3));
impl_dispatch!(__const_fn_specialization_dispatch5(a0: A0, a1: A1, a2: A2, a3: A3, a4: A4));
impl_dispatch!(__const_fn_specialization_dispatch6(a0: A0, a1: A1, a2: A2, a3: A3, a4: A4, a5: A5));
impl_dispatch!(
    __const_fn_specialization_dispatch7(a0: A0, a1: A1, a2: A2, a3: A3, a4: A4, a5: A5, a6: A6)
);
impl_dispatch!(
    __const_fn_specialization_dispatch8(
        a0: A0,
        a1: A1,
        a2: A2,
        a3: A3,
        a4: A4,
        a5: A5,
        a6: A6,
        a7: A7,
    )
);
impl_dispatch!(
    __const_fn_specialization_dispatch9(
        a0: A0,
        a1: A1,
        a2: A2,
        a3: A3,
        a4: A4,
        a5: A5,
        a6: A6,
        a7: A7,
        a8: A8,
    )
);
impl_dispatch!(
    __const_fn_specialization_dispatch10(
        a0: A0,
        a1: A1,
        a2: A2,
        a3: A3,
        a4: A4,
        a5: A5,
        a6: A6,
        a7: A7,
        a8: A8,
        a9: A9,
    )
);
impl_dispatch!(
    __const_fn_specialization_dispatch11(
        a0: A0,
        a1: A1,
        a2: A2,
        a3: A3,
        a4: A4,
        a5: A5,
        a6: A6,
        a7: A7,
        a8: A8,
        a9: A9,
        a10: A10,
    )
);
impl_dispatch!(
    __const_fn_specialization_dispatch12(
        a0: A0,
        a1: A1,
        a2: A2,
        a3: A3,
        a4: A4,
        a5: A5,
        a6: A6,
        a7: A7,
        a8: A8,
        a9: A9,
        a10: A10,
        a11: A11,
    )
);
