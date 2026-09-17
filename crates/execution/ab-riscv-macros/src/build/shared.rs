use crate::build::state::{KnownEnumDefinition, State};
use quote::ToTokens;
use std::collections::{HashSet, VecDeque};
use std::mem;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, ItemEnum, Meta, Token, WherePredicate, parse_quote};

pub(super) fn collect_all_dependencies<InitialDependencies>(
    state: &State,
    initial_dependencies: InitialDependencies,
) -> Result<Vec<(Ident, &KnownEnumDefinition)>, Ident>
where
    InitialDependencies: Iterator<Item = Ident>,
{
    let mut already_inserted = HashSet::new();
    let mut all_dependencies = Vec::new();
    let mut new_dependencies = VecDeque::from_iter(initial_dependencies);

    while let Some(dependency_enum_name) = new_dependencies.pop_front() {
        let Some(dependency_enum_definition) =
            state.get_known_enum_definition(&dependency_enum_name)
        else {
            return Err(dependency_enum_name);
        };

        if !already_inserted.insert(dependency_enum_name.clone()) {
            continue;
        }

        all_dependencies.push((dependency_enum_name, dependency_enum_definition));
        new_dependencies.extend(
            dependency_enum_definition
                .direct_dependencies
                .iter()
                .cloned(),
        );
    }

    Ok(all_dependencies)
}

/// Strips `[const]` from `where` predicates.
///
/// Used both for a non-`const` implementation that inherits arms from `const` ones and for the
/// threaded implementation, which is never `const`.
pub(super) fn strip_const_where_predicates(predicates: &mut Punctuated<WherePredicate, Token![,]>) {
    for predicate in predicates {
        if let WherePredicate::Type(predicate_type) = predicate {
            // TODO: `BRCONST` is a hack that allows `syn` to parse unstable Rust syntax
            //  around const traits and such. It will change to a proper modifier once
            //  stabilized
            if predicate_type.bounds.first() == Some(&parse_quote! { BRCONST }) {
                predicate_type.bounds = mem::take(&mut predicate_type.bounds)
                    .into_iter()
                    .skip(1)
                    .collect();
            }
        }
    }
}

/// Integer type the discriminant of an instruction enum is stored in, as pinned by its
/// `#[repr(...)]`.
///
/// Threaded dispatch reads the discriminant out of the instruction to index a table of handlers
/// with it, which is only meaningful for an enum whose tag is an integer of a known type stored at
/// the beginning of the value. `u8` and `u16` are the only ones accepted because the index is
/// widened to `usize` with `usize::from()`, which is lossless on every target, and 16 bits is
/// already far more instructions than any RISC-V extension combination needs.
pub(super) fn enum_discriminant_type(item_enum: &ItemEnum) -> anyhow::Result<Ident> {
    for attr in &item_enum.attrs {
        if !attr.path().is_ident("repr") {
            continue;
        }

        let maybe_discriminant_type = attr
            .parse_args_with(|input: ParseStream<'_>| {
                let nested = input.parse_terminated(Meta::parse, Token![,])?;
                Ok(nested.iter().find_map(|meta| {
                    let ident = meta.path().get_ident()?;
                    (ident == "u8" || ident == "u16").then(|| ident.clone())
                }))
            })
            .unwrap_or_default();

        if let Some(discriminant_type) = maybe_discriminant_type {
            return Ok(discriminant_type);
        }
    }

    Err(anyhow::anyhow!(
        "Instruction enum `{}` must be `#[repr(u8)]` or `#[repr(u16)]` for threaded dispatch to \
        index its handler table with the discriminant, found: {}",
        item_enum.ident,
        item_enum
            .attrs
            .iter()
            .map(|attr| attr.to_token_stream().to_string())
            .collect::<Vec<_>>()
            .join(" "),
    ))
}
