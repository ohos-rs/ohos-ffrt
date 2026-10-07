use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::format_ident;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{
    Expr, ExprLit, ItemFn, Lit, LitInt, LitStr, MetaNameValue, Result, Token, parse_macro_input,
};

#[derive(Default)]
struct RuntimeArgs {
    flavor: Option<String>,
    worker_threads: Option<usize>,
}

impl Parse for RuntimeArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let item: MetaNameValue = input.parse()?;
            let name = item
                .path
                .get_ident()
                .ok_or_else(|| syn::Error::new_spanned(&item.path, "expected an option name"))?;
            match name.to_string().as_str() {
                "flavor" => {
                    let Expr::Lit(ExprLit {
                        lit: Lit::Str(value),
                        ..
                    }) = item.value
                    else {
                        return Err(syn::Error::new_spanned(item, "flavor must be a string"));
                    };
                    args.flavor = Some(value.value());
                }
                "worker_threads" => {
                    let Expr::Lit(ExprLit {
                        lit: Lit::Int(value),
                        ..
                    }) = item.value
                    else {
                        return Err(syn::Error::new_spanned(
                            item,
                            "worker_threads must be an integer",
                        ));
                    };
                    args.worker_threads = Some(parse_usize(&value)?);
                }
                option => {
                    return Err(syn::Error::new_spanned(
                        item,
                        format!("unknown runtime option `{option}`"),
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(args)
    }
}

fn parse_usize(value: &LitInt) -> Result<usize> {
    let parsed = value.base10_parse::<usize>()?;
    if parsed == 0 {
        return Err(syn::Error::new_spanned(
            value,
            "worker_threads must be positive",
        ));
    }
    Ok(parsed)
}

#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args, input, false)
}

#[proc_macro_attribute]
pub fn test(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(args, input, true)
}

fn expand(args: TokenStream, input: TokenStream, is_test: bool) -> TokenStream {
    let args = parse_macro_input!(args as RuntimeArgs);
    let mut function = parse_macro_input!(input as ItemFn);

    if function.sig.asyncness.take().is_none() {
        return syn::Error::new_spanned(function.sig.fn_token, "the async keyword is missing")
            .into_compile_error()
            .into();
    }
    if (is_test || function.sig.ident == "main") && !function.sig.inputs.is_empty() {
        return syn::Error::new_spanned(
            &function.sig.inputs,
            "runtime functions cannot accept arguments",
        )
        .into_compile_error()
        .into();
    }

    let default_flavor = if is_test {
        "current_thread"
    } else {
        "multi_thread"
    };
    let flavor = args.flavor.as_deref().unwrap_or(default_flavor);
    let runtime_crate = match crate_name("ffrt") {
        Ok(FoundCrate::Itself) => quote!(::ffrt),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{}", name, span = Span::call_site());
            quote!(::#name)
        }
        Err(_) => quote!(::ffrt),
    };
    let builder = match flavor {
        "current_thread" => quote!(#runtime_crate::runtime::Builder::new_current_thread()),
        "multi_thread" => quote!(#runtime_crate::runtime::Builder::new_multi_thread()),
        _ => {
            return syn::Error::new_spanned(
                LitStr::new(flavor, proc_macro2::Span::call_site()),
                "flavor must be `current_thread` or `multi_thread`",
            )
            .into_compile_error()
            .into();
        }
    };
    if flavor == "current_thread" && args.worker_threads.is_some() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "worker_threads requires the multi_thread flavor",
        )
        .into_compile_error()
        .into();
    }

    let worker_threads = args.worker_threads.map(|count| {
        quote! {
            builder.worker_threads(#count);
        }
    });
    let body = function.block;
    function.block = Box::new(syn::parse_quote!({
        let mut builder = #builder;
        builder.enable_all();
        #worker_threads
        builder
            .build()
            .expect("failed to build FFRT runtime")
            .block_on(async move #body)
    }));
    if is_test {
        function
            .attrs
            .push(syn::parse_quote!(#[::core::prelude::v1::test]));
    }

    quote!(#function).into()
}

mod select;

#[doc(hidden)]
#[proc_macro]
pub fn __select(input: TokenStream) -> TokenStream {
    select::expand(parse_macro_input!(input as select::Select)).into()
}
