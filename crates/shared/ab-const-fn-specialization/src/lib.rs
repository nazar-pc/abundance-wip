#![feature(macro_attr)]
#![feature(macro_metavar_expr_concat)]

//! Declarative attribute macro implementing "const fn specialization" as described in
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
//! This crate is built on top of two unstable compiler features that are not yet stabilized:
//! * declarative (`macro_rules!`-based) attribute macros, tracked in <https://github.com/rust-lang/rust/issues/143547>
//!   (RFC 3697), feature gate `macro_attr`
//! * the `${concat(...)}` meta-variable expression used to generate the hidden functions' names,
//!   tracked in <https://github.com/rust-lang/rust/issues/124225>, feature gate
//!   `macro_metavar_expr_concat`
//!
//! Because `#[const_fn_specialization]` expands to code that calls
//! [`core::intrinsics::const_eval_select`], **every crate that uses this macro** must enable the
//! following features itself (feature gates are per-crate, they are not inherited through macro
//! expansion):
//!
//! ```ignore
//! #![feature(macro_attr)]
//! #![feature(macro_metavar_expr_concat)]
//! #![feature(const_eval_select)]
//! #![feature(core_intrinsics)]
//! #![allow(internal_features)]
//! ```
//!
//! # Example
//!
//! ```
//! # #![feature(macro_attr)]
//! # #![feature(macro_metavar_expr_concat)]
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
//! # Limitations
//!
//! * Both annotated items must appear in the same module and share the same name, visibility,
//!   parameter list and return type; the macro has no way to check this beyond what the compiler
//!   itself enforces once both expansions are in scope.
//! * Function parameters must be simple `name: Type` bindings (no patterns, no `mut`, no `impl
//!   Trait` etc.) because parameter names are reused to build the argument tuple passed to
//!   [`core::intrinsics::const_eval_select`].
//! * Generic functions and `where` clauses are not supported by this version of the macro:
//!   `macro_rules!` cannot unambiguously capture a `<...>` generics list as raw tokens, and
//!   properly forwarding turbofish arguments to `const_eval_select`'s two branches is out of scope
//!   here.
//! * `self`/method receivers are not supported, only free functions.

/// See the [crate-level documentation](crate) for usage and limitations.
#[macro_export]
macro_rules! const_fn_specialization {
    attr() (
        $(#[$meta:meta])*
        $vis:vis const fn $name:ident ( $($pname:ident : $pty:ty),* $(,)? ) $(-> $ret:ty)?
        $body:block
    ) => {
        $(#[$meta])*
        #[doc(hidden)]
        $vis const fn ${concat(__const_fn_specialization_const_, $name)} ( $($pname : $pty),* ) $(-> $ret)?
        $body
    };

    attr() (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident ( $($pname:ident : $pty:ty),* $(,)? ) $(-> $ret:ty)?
        $body:block
    ) => {
        $(#[$meta])*
        #[doc(hidden)]
        $vis fn ${concat(__const_fn_specialization_rt_, $name)} ( $($pname : $pty),* ) $(-> $ret)?
        $body

        $vis const fn $name ( $($pname : $pty),* ) $(-> $ret)? {
            // SAFETY: both branches are meant to be semantically equivalent implementations of
            // the same function, one of which happens to be usable in `const` context and the
            // other one isn't.
            unsafe {
                ::core::intrinsics::const_eval_select(
                    ($($pname,)*),
                    ${concat(__const_fn_specialization_const_, $name)},
                    ${concat(__const_fn_specialization_rt_, $name)},
                )
            }
        }
    };
}
