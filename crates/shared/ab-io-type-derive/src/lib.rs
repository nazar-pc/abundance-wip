use proc_macro2::{Group, Ident, Literal, Span, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use std::iter;
use syn::spanned::Spanned;
use syn::token::Paren;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Error, Fields, LitInt, parenthesized,
    parse_macro_input,
};

/// Options of all `#[repr(..)]` attributes of a type, each with the attribute it is in
#[derive(Default)]
struct Repr<'a> {
    c: Option<&'a Attribute>,
    transparent: Option<&'a Attribute>,
    u8: Option<&'a Attribute>,
    /// Integer types other than `u8`
    other_integer: Option<&'a Attribute>,
    /// `align(N)`, `packed` or `packed(N)`
    align_or_packed: Option<&'a Attribute>,
}

#[proc_macro_derive(TrivialType)]
pub fn trivial_type_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    if !input.generics.params.is_empty() {
        return Error::new(
            input.ident.span(),
            "`TrivialType` can't be derived on generic types",
        )
        .into_compile_error()
        .into();
    }

    let mut repr_attrs = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("repr"))
        .peekable();

    let Some(&first_repr_attr) = repr_attrs.peek() else {
        return Error::new(input.ident.span(), "`TrivialType` requires `#[repr(..)]`")
            .to_compile_error()
            .into();
    };

    // All `#[repr(..)]` attributes are combined by the compiler
    let mut repr = Repr::default();
    for repr_attr in repr_attrs {
        if let Err(error) = parse_repr(&mut repr, repr_attr) {
            return error.to_compile_error().into();
        }
    }

    if let Some(repr_attr) = repr.align_or_packed {
        return Error::new_spanned(
            repr_attr,
            "`TrivialType` doesn't allow `#[repr(align(N))]` or `#[repr(packed(N))]`",
        )
        .to_compile_error()
        .into();
    }

    let type_name = &input.ident;

    let output = match &input.data {
        Data::Struct(data_struct) => {
            if repr.c.is_none() && repr.transparent.is_none() {
                return Error::new_spanned(
                    repr.u8.or(repr.other_integer).unwrap_or(first_repr_attr),
                    "`TrivialType` on structs requires `#[repr(C)]` or `#[repr(transparent)]`",
                )
                .into_compile_error()
                .into();
            }
            let field_types = data_struct
                .fields
                .iter()
                .map(|field| &field.ty)
                .collect::<Vec<_>>();

            let struct_metadata = match generate_struct_metadata(type_name, data_struct) {
                Ok(struct_metadata) => struct_metadata,
                Err(error) => {
                    return error.to_compile_error().into();
                }
            };

            quote! {
                const _: () = {
                    // Assert statically that there is no unexpected padding that would be left
                    // uninitialized and unsound to access
                    assert!(
                        0 == (
                            ::core::mem::size_of::<#type_name>()
                            #(- ::core::mem::size_of::<#field_types>() )*
                        ),
                        "Struct must not have implicit padding. Consider reordering fields, adding \
                        `padding: [u8; N]` field where necessary or use `Unaligned<T>` wrapper for \
                        types with larger alignment to reduce it to one byte."
                    );

                    // Assert that type doesn't exceed 32-bit size limit
                    assert!(
                        u32::MAX as ::core::primitive::usize >= ::core::mem::size_of::<#type_name>(),
                        "Type size must be smaller than 2^32"
                    );

                    // Ensure metadata decodes completely, and capacity and alignment are correctly
                    // decoded from it
                    let (type_details, remaining_metadata) =
                        ::ab_io_type::metadata::IoTypeMetadataKind::type_details(
                            <#type_name as ::ab_io_type::trivial_type::TrivialType>::METADATA,
                        )
                            .expect("Statically correct metadata; qed");
                    assert!(remaining_metadata.is_empty());
                    assert!(size_of::<#type_name>() == type_details.recommended_capacity as ::core::primitive::usize);
                    assert!(align_of::<#type_name>() == type_details.alignment.get() as ::core::primitive::usize);
                };

                #[automatically_derived]
                unsafe impl ::ab_io_type::trivial_type::TrivialType for #type_name
                where
                    #( #field_types: ::ab_io_type::trivial_type::TrivialType, )*
                {
                    const METADATA: &[::core::primitive::u8] = #struct_metadata;
                }
            }
        }
        Data::Enum(data_enum) => {
            // Require defined size of the discriminant instead of allowing compiler to guess
            if repr.u8.is_none() || repr.other_integer.is_some() {
                return Error::new_spanned(
                    repr.other_integer
                        .or(repr.c)
                        .or(repr.transparent)
                        .unwrap_or(first_repr_attr),
                    "`TrivialType` derive for enums only supports `#[repr(u8)]`, ambiguous \
                    or larger discriminant size is not allowed",
                )
                .to_compile_error()
                .into();
            }
            // With `#[repr(u8)]` each variant is laid out like a `#[repr(C)]` struct with the
            // discriminant as the first field, which padding assertions rely on, `#[repr(C, u8)]`
            // has a different layout
            if let Some(repr_attr) = repr.c.or(repr.transparent) {
                return Error::new_spanned(
                    repr_attr,
                    "`TrivialType` derive for enums only supports `#[repr(u8)]` without other \
                    options, `#[repr(C, u8)]` has different offsets of fields",
                )
                .to_compile_error()
                .into();
            }

            // Metadata identifies variants by their index and doesn't record discriminants, so
            // explicit discriminants must be equal to the index. An implicit discriminant is zero
            // for the first variant and one more than the previous discriminant otherwise, so it is
            // equal to the index too when all explicit discriminants are. `#[repr(u8)]` doesn't
            // allow more than 256 variants, so each variant gets an index.
            let discriminant_assertions = data_enum
                .variants
                .iter()
                .zip(0..=u8::MAX)
                .filter_map(|(variant, index)| {
                    let (_eq_token, discriminant) = variant.discriminant.as_ref()?;
                    // Errors point at the discriminant
                    let span = Span::call_site().located_at(discriminant.span());
                    // Discriminants are constant expressions that can refer to `Self`, which
                    // doesn't exist outside the enum. Only `Self` written in the discriminant is
                    // replaced, `Self` produced by macros or used in nested impls is not supported.
                    let discriminant = replace_self(discriminant.to_token_stream(), type_name);
                    let message = format!(
                        "Discriminant of `{}` must be equal to its index {index}, variants are \
                        identified by their index in `TrivialType` metadata",
                        variant.ident
                    );

                    Some(quote_spanned! {span=>
                        // The index goes first, so the discriminant is inferred as `u8` like in
                        // the enum definition
                        const _: () = assert!(#index == (#discriminant), #message);
                    })
                })
                .collect::<Vec<_>>();

            let repr_numeric = format_ident!("u8");

            let field_types = data_enum
                .variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .map(|field| &field.ty)
                .collect::<Vec<_>>();

            let padding_assertions = data_enum.variants.iter().map(|variant| {
                let field_types = variant.fields.iter().map(|field| &field.ty);

                quote! {
                    // Assert statically that there is no unexpected padding that would be left
                    // uninitialized and unsound to access
                    assert!(
                        0 == (
                            ::core::mem::size_of::<#type_name>()
                            - ::core::mem::size_of::<::core::primitive::#repr_numeric>()
                            #(- ::core::mem::size_of::<#field_types>() )*
                        ),
                        "Enum must not have implicit padding. Consider reordering fields, adding \
                        `padding: [u8; N]` field where necessary or use `Unaligned<T>` wrapper for \
                        types with larger alignment to reduce it to one byte."
                    );
                }
            });

            let enum_metadata = match generate_enum_metadata(type_name, data_enum) {
                Ok(struct_metadata) => struct_metadata,
                Err(error) => {
                    return error.to_compile_error().into();
                }
            };

            quote! {
                #( #discriminant_assertions )*

                const _: () = {
                    #( #padding_assertions )*

                    // Assert that type doesn't exceed 32-bit size limit
                    assert!(
                        u32::MAX as ::core::primitive::usize >= ::core::mem::size_of::<#type_name>(),
                        "Type size must be smaller than 2^32"
                    );

                    // Ensure metadata decodes completely, and capacity and alignment are correctly
                    // decoded from it
                    let (type_details, remaining_metadata) =
                        ::ab_io_type::metadata::IoTypeMetadataKind::type_details(
                            <#type_name as ::ab_io_type::trivial_type::TrivialType>::METADATA,
                        )
                            .expect("Statically correct metadata; qed");
                    assert!(remaining_metadata.is_empty());
                    assert!(size_of::<#type_name>() == type_details.recommended_capacity as ::core::primitive::usize);
                    assert!(align_of::<#type_name>() == type_details.alignment.get() as ::core::primitive::usize);
                };

                #[automatically_derived]
                unsafe impl ::ab_io_type::trivial_type::TrivialType for #type_name
                where
                    #( #field_types: ::ab_io_type::trivial_type::TrivialType, )*
                {
                    const METADATA: &[::core::primitive::u8] = #enum_metadata;
                }
            }
        }
        Data::Union(data_union) => {
            return Error::new(
                data_union.union_token.span(),
                "`TrivialType` can be derived for structs and enums, but not unions",
            )
            .to_compile_error()
            .into();
        }
    };

    output.into()
}

/// Parse options of a `#[repr(..)]` attribute into `repr`, which can contain options of other
/// `#[repr(..)]` attributes of the same type
fn parse_repr<'a>(repr: &mut Repr<'a>, repr_attr: &'a Attribute) -> Result<(), Error> {
    // Based on https://docs.rs/syn/2.0.93/syn/struct.Attribute.html#method.parse_nested_meta
    repr_attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("C") {
            repr.c = Some(repr_attr);
            return Ok(());
        }
        if meta.path.is_ident("u8") {
            repr.u8 = Some(repr_attr);
            return Ok(());
        }
        if ["u16", "u32", "u64", "u128"]
            .into_iter()
            .any(|integer| meta.path.is_ident(integer))
        {
            repr.other_integer = Some(repr_attr);
            return Ok(());
        }
        if meta.path.is_ident("transparent") {
            repr.transparent = Some(repr_attr);
            return Ok(());
        }

        // #[repr(align(N))]
        if meta.path.is_ident("align") {
            let content;
            parenthesized!(content in meta.input);
            let lit = content.parse::<LitInt>()?;
            lit.base10_parse::<usize>()?;
            repr.align_or_packed = Some(repr_attr);
            return Ok(());
        }

        // #[repr(packed)] or #[repr(packed(N))], omitted N means 1
        if meta.path.is_ident("packed") {
            if meta.input.peek(Paren) {
                let content;
                parenthesized!(content in meta.input);
                let lit = content.parse::<LitInt>()?;
                lit.base10_parse::<usize>()?;
            }
            repr.align_or_packed = Some(repr_attr);
            return Ok(());
        }

        Err(meta.error("Unsupported `#[repr(..)]`"))
    })
}

/// Replace `Self` in `tokens` with `type_name`, so that an expression from the definition of the
/// type can be used outside of it
fn replace_self(tokens: TokenStream, type_name: &Ident) -> TokenStream {
    tokens
        .into_iter()
        .map(|token| match token {
            TokenTree::Ident(ident) if ident == "Self" => {
                let mut type_name = type_name.clone();
                type_name.set_span(ident.span());
                TokenTree::Ident(type_name)
            }
            TokenTree::Group(group) => {
                let mut new_group =
                    Group::new(group.delimiter(), replace_self(group.stream(), type_name));
                new_group.set_span(group.span());
                TokenTree::Group(new_group)
            }
            token => token,
        })
        .collect()
}

fn generate_struct_metadata(ident: &Ident, data_struct: &DataStruct) -> Result<TokenStream, Error> {
    let num_fields = data_struct.fields.len();
    let (io_type_metadata, with_num_fields) = if matches!(data_struct.fields, Fields::Named(_)) {
        match num_fields {
            0..=10 => (format_ident!("Struct{num_fields}"), false),
            _ => (format_ident!("Struct"), true),
        }
    } else {
        match num_fields {
            1..=10 => (format_ident!("TupleStruct{num_fields}"), false),
            _ => (format_ident!("TupleStruct"), true),
        }
    };
    let inner_struct_metadata =
        generate_inner_struct_metadata(ident, &data_struct.fields, with_num_fields)
            .collect::<Result<Vec<_>, _>>()?;

    // Encodes the following:
    // * Type: struct
    // * The rest as inner struct metadata
    Ok(quote! {{
        #[inline(always)]
        const fn metadata() -> (
            [::core::primitive::u8; ::ab_io_type::metadata::MAX_METADATA_CAPACITY],
            usize,
        )
        {
            ::ab_io_type::metadata::concat_metadata_sources(&[
                &[::ab_io_type::metadata::IoTypeMetadataKind::#io_type_metadata as ::core::primitive::u8],
                #( #inner_struct_metadata )*
            ])
        }

        // Strange syntax to allow Rust to extend the lifetime of metadata scratch automatically
        metadata()
            .0
            .split_at(metadata().1)
            .0
    }})
}

fn generate_enum_metadata(ident: &Ident, data_enum: &DataEnum) -> Result<TokenStream, Error> {
    let type_name_string = ident.to_string();
    let type_name_bytes = type_name_string.as_bytes();

    let type_name_bytes_len = u8::try_from(type_name_bytes.len()).map_err(|_error| {
        Error::new(
            ident.span(),
            format!(
                "Name of the enum must not be more than {} bytes in length",
                u8::MAX
            ),
        )
    })?;
    let num_variants = u8::try_from(data_enum.variants.len()).map_err(|_error| {
        Error::new(
            ident.span(),
            format!("Enum must not have more than {} variants", u8::MAX),
        )
    })?;
    // Variants without fields in an enum with fields are encoded as variants with zero fields
    let with_fields = data_enum
        .variants
        .iter()
        .any(|variant| !variant.fields.is_empty());
    let enum_type = if with_fields { "Enum" } else { "EnumNoFields" };
    let (io_type_metadata, with_num_variants) = match num_variants {
        1..=10 => (format_ident!("{enum_type}{num_variants}"), false),
        _ => (format_ident!("{enum_type}"), true),
    };

    // Encodes the following:
    // * Type: enum
    // * Length of enum name in bytes (u8)
    // * Enum name as UTF-8 bytes
    // * Number of variants (u8, if requested)
    let enum_metadata_header = {
        let enum_metadata_header = [Literal::u8_unsuffixed(type_name_bytes_len)]
            .into_iter()
            .chain(
                type_name_bytes
                    .iter()
                    .map(|&char| Literal::byte_character(char)),
            )
            .chain(with_num_variants.then_some(Literal::u8_unsuffixed(num_variants)));

        quote! {
            &[
                ::ab_io_type::metadata::IoTypeMetadataKind::#io_type_metadata as ::core::primitive::u8,
                #( #enum_metadata_header, )*
            ]
        }
    };

    // Encodes each variant as inner struct
    let inner = data_enum
        .variants
        .iter()
        .flat_map(|variant| {
            variant
                .fields
                .iter()
                .find_map(|field| {
                    if field.ident.is_none() {
                        Some(Err(Error::new(
                            field.span(),
                            "Variant must have named fields",
                        )))
                    } else {
                        None
                    }
                })
                .into_iter()
                .chain(generate_inner_struct_metadata(
                    &variant.ident,
                    &variant.fields,
                    with_fields,
                ))
        })
        .collect::<Result<Vec<TokenStream>, Error>>()?;

    Ok(quote! {{
        #[inline(always)]
        const fn metadata() -> (
            [::core::primitive::u8; ::ab_io_type::metadata::MAX_METADATA_CAPACITY],
            usize,
        )
        {
            ::ab_io_type::metadata::concat_metadata_sources(&[
                #enum_metadata_header,
                #( #inner )*
            ])
        }

        // Strange syntax to allow Rust to extend the lifetime of metadata scratch automatically
        metadata()
            .0
            .split_at(metadata().1)
            .0
    }})
}

fn generate_inner_struct_metadata<'a>(
    ident: &'a Ident,
    fields: &'a Fields,
    with_num_fields: bool,
) -> impl Iterator<Item = Result<TokenStream, Error>> + 'a {
    iter::once_with(move || generate_inner_struct_metadata_header(ident, fields, with_num_fields))
        .chain(generate_fields_metadata(fields))
}

fn generate_inner_struct_metadata_header(
    ident: &Ident,
    fields: &Fields,
    with_num_fields: bool,
) -> Result<TokenStream, Error> {
    let ident_string = ident.to_string();
    let ident_bytes = ident_string.as_bytes();

    let ident_bytes_len = u8::try_from(ident_bytes.len()).map_err(|_error| {
        Error::new(
            ident.span(),
            format!(
                "Identifier must not be more than {} bytes in length",
                u8::MAX
            ),
        )
    })?;
    let num_fields = u8::try_from(fields.len()).map_err(|_error| {
        Error::new(
            fields.span(),
            format!("Must not have more than {} fields", u8::MAX),
        )
    })?;

    // Encodes the following:
    // * Length of identifier in bytes (u8)
    // * Identifier as UTF-8 bytes
    // * Number of fields (u8, if requested)
    Ok({
        let struct_metadata_header = [Literal::u8_unsuffixed(ident_bytes_len)]
            .into_iter()
            .chain(
                ident_bytes
                    .iter()
                    .map(|&char| Literal::byte_character(char)),
            )
            .chain(with_num_fields.then_some(Literal::u8_unsuffixed(num_fields)));

        quote! {
            &[#( #struct_metadata_header, )*],
        }
    })
}

fn generate_fields_metadata(
    fields: &Fields,
) -> impl Iterator<Item = Result<TokenStream, Error>> + '_ {
    // Encodes the following for each field:
    // * Length of the field name in bytes (u8, if not tuple)
    // * Field name as UTF-8 bytes (if not tuple)
    // * Recursive metadata of the field's type
    fields.iter().map(move |field| {
        let field_metadata = if let Some(field_name) = &field.ident {
            let field_name_string = field_name.to_string();
            let field_name_bytes = field_name_string.as_bytes();
            let field_name_len = u8::try_from(field_name_bytes.len()).map_err(|_error| {
                Error::new(
                    field.span(),
                    format!(
                        "Name of the field must not be more than {} bytes in length",
                        u8::MAX
                    ),
                )
            })?;

            let field_metadata = [Literal::u8_unsuffixed(field_name_len)].into_iter().chain(
                field_name_bytes
                    .iter()
                    .map(|&char| Literal::byte_character(char)),
            );

            Some(quote! { #( #field_metadata, )* })
        } else {
            None
        };
        let field_type = &field.ty;

        Ok(quote! {
            &[ #field_metadata ],
            <#field_type as ::ab_io_type::trivial_type::TrivialType>::METADATA,
        })
    })
}
