//! Implements the `#[const_fn_specialization]` attribute macro, see `ab-const-fn-specialization`
//! crate for public documentation and usage.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Error, FnArg, GenericParam, Generics, ItemFn, parse_macro_input};

/// Matches the number of `__const_fn_specialization_dispatchN` helpers defined in
/// `ab-const-fn-specialization`.
const MAX_ARITY: usize = 12;

/// See `ab-const-fn-specialization` crate for documentation.
#[proc_macro_attribute]
pub fn const_fn_specialization(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return Error::new_spanned(
            TokenStream2::from(attr),
            "`#[const_fn_specialization]` does not take any arguments",
        )
        .into_compile_error()
        .into();
    }

    let item_fn = parse_macro_input!(item as ItemFn);

    expand(item_fn)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn expand(mut item_fn: ItemFn) -> syn::Result<TokenStream2> {
    if item_fn.sig.receiver().is_some() {
        return Err(Error::new_spanned(
            &item_fn.sig,
            "`#[const_fn_specialization]` only supports free functions, not methods",
        ));
    }

    let name = item_fn.sig.ident.clone();
    let is_const = item_fn.sig.constness.is_some();

    if is_const {
        item_fn.sig.ident = format_ident!("__const_fn_specialization_const_{name}");
        item_fn.attrs.push(syn::parse_quote!(#[doc(hidden)]));
        return Ok(quote! { #item_fn });
    }

    let original_attrs = item_fn.attrs.clone();
    let vis = item_fn.vis.clone();
    let generics = item_fn.sig.generics.clone();
    let (impl_generics, _ty_generics, where_clause) = generics.split_for_impl();
    let return_type = item_fn.sig.output.clone();

    if item_fn.sig.inputs.len() > MAX_ARITY {
        return Err(Error::new_spanned(
            &item_fn.sig.inputs,
            format!("`#[const_fn_specialization]` supports at most {MAX_ARITY} parameters"),
        ));
    }

    let mut dispatch_params = Vec::new();
    let mut arg_names = Vec::new();
    for (index, input) in item_fn.sig.inputs.iter().enumerate() {
        let FnArg::Typed(pat_type) = input else {
            // `self`/`&self`/`&mut self` receivers were already rejected above.
            unreachable!("only typed function arguments are expected here");
        };
        let arg_name = format_ident!("__const_fn_specialization_arg{index}");
        let ty = &pat_type.ty;
        dispatch_params.push(quote! { #arg_name: #ty });
        arg_names.push(arg_name);
    }
    let dispatch_fn_name = format_ident!("__const_fn_specialization_dispatch{}", arg_names.len());

    // Lifetime parameters of a `fn` are late-bound and can't be specified via turbofish (that
    // would be a `E0794` error), so only type/const parameters are forwarded here; they are
    // inferred at each call site from `arg_names` and `return_type` instead.
    let turbofish_generics = Generics {
        params: generics
            .params
            .iter()
            .filter(|param| !matches!(param, GenericParam::Lifetime(_)))
            .cloned()
            .collect(),
        ..Generics::default()
    };
    let ty_generics_turbofish = turbofish_generics.split_for_impl().1.as_turbofish();

    let const_impl_name = format_ident!("__const_fn_specialization_const_{name}");
    let rt_impl_name = format_ident!("__const_fn_specialization_rt_{name}");
    let const_impl_path = quote! { #const_impl_name #ty_generics_turbofish };
    let rt_impl_path = quote! { #rt_impl_name #ty_generics_turbofish };

    item_fn.sig.ident = rt_impl_name;
    item_fn.attrs.push(syn::parse_quote!(#[doc(hidden)]));

    Ok(quote! {
        #item_fn

        #(#original_attrs)*
        #vis const fn #name #impl_generics ( #(#dispatch_params),* ) #return_type #where_clause {
            // SAFETY: both branches are meant to be semantically equivalent implementations of
            // the same function, one of which happens to be usable in `const` context and the
            // other one isn't; this is a documented, but unenforceable, contract of
            // `const_eval_select` itself, which `__const_fn_specialization_dispatchN` wraps.
            unsafe {
                ::ab_const_fn_specialization::#dispatch_fn_name(
                    #(#arg_names,)*
                    #const_impl_path,
                    #rt_impl_path,
                )
            }
        }
    })
}
