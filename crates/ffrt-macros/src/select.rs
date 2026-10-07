use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::visit_mut::VisitMut;
use syn::{Expr, Pat, Result, Token, parenthesized};

struct Branch {
    pattern: Pat,
    future: Expr,
    condition: Option<Expr>,
    handler: Expr,
}

pub(crate) struct Select {
    start: Expr,
    biased: bool,
    branches: Vec<Branch>,
    otherwise: Option<Expr>,
}

impl Parse for Select {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let start;
        parenthesized!(start in input);
        let start = start.parse()?;
        input.parse::<Token![;]>()?;
        let biased = if input.peek(syn::Ident) && input.peek2(Token![;]) {
            let keyword: syn::Ident = input.parse()?;
            if keyword != "biased" {
                return Err(syn::Error::new_spanned(keyword, "expected biased"));
            }
            input.parse::<Token![;]>()?;
            true
        } else {
            false
        };
        let mut branches = Vec::new();
        let mut otherwise = None;
        while !input.is_empty() {
            if input.peek(Token![else]) {
                input.parse::<Token![else]>()?;
                input.parse::<Token![=>]>()?;
                otherwise = Some(input.parse()?);
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                if !input.is_empty() {
                    return Err(input.error("else must be the last branch"));
                }
                break;
            }
            let pattern = Pat::parse_multi_with_leading_vert(input)?;
            input.parse::<Token![=]>()?;
            let future = input.parse()?;
            let condition = if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
                input.parse::<Token![if]>()?;
                Some(input.parse()?)
            } else {
                None
            };
            input.parse::<Token![=>]>()?;
            let handler: Expr = input.parse()?;
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else if !input.is_empty()
                && !matches!(
                    handler,
                    Expr::Block(_)
                        | Expr::If(_)
                        | Expr::Match(_)
                        | Expr::Loop(_)
                        | Expr::While(_)
                        | Expr::ForLoop(_)
                        | Expr::Unsafe(_)
                )
            {
                return Err(input.error("expected comma after select handler"));
            }
            branches.push(Branch {
                pattern,
                future,
                condition,
                handler,
            });
        }
        if branches.is_empty() && otherwise.is_none() {
            return Err(input.error("select requires a branch"));
        }
        if branches.len() > 64 {
            return Err(input.error("select supports at most 64 branches"));
        }
        Ok(Self {
            start,
            biased,
            branches,
            otherwise,
        })
    }
}

struct CleanPattern;
impl VisitMut for CleanPattern {
    fn visit_pat_ident_mut(&mut self, pattern: &mut syn::PatIdent) {
        pattern.by_ref = None;
        pattern.mutability = None;
        syn::visit_mut::visit_pat_ident_mut(self, pattern);
    }
}

pub(crate) fn expand(select: Select) -> TokenStream {
    let Select {
        start,
        biased,
        branches,
        otherwise,
    } = select;
    let otherwise = otherwise
        .map(|expr| quote!(#expr))
        .unwrap_or_else(|| quote!(panic!("all select branches were disabled")));
    if branches.is_empty() {
        return quote!({ #otherwise });
    }
    let count = branches.len();
    let variants: Vec<_> = (0..count).map(|i| format_ident!("Branch{i}")).collect();
    let types: Vec<_> = (0..count).map(|i| format_ident!("T{i}")).collect();
    let indexes: Vec<_> = (0..count).map(syn::Index::from).collect();
    let numbers: Vec<_> = (0..count).collect();
    let futures = branches.iter().map(|branch| &branch.future);
    let conditions = branches.iter().map(|branch| {
        branch
            .condition
            .as_ref()
            .map(|expr| quote!(#expr))
            .unwrap_or_else(|| quote!(true))
    });
    let patterns: Vec<_> = branches.iter().map(|branch| &branch.pattern).collect();
    let checked_patterns: Vec<_> = patterns
        .iter()
        .map(|pattern| {
            let mut pattern = (*pattern).clone();
            CleanPattern.visit_pat_mut(&mut pattern);
            pattern
        })
        .collect();
    let handlers = branches.iter().map(|branch| &branch.handler);
    let start = if biased {
        quote!(0usize)
    } else {
        quote!((#start)(#count))
    };
    // Keep generated locals distinct from identically named caller bindings.
    let output_enum = format_ident!("__FfrtSelectOutput", span = proc_macro2::Span::mixed_site());
    let result_ident = format_ident!("__ffrt_result", span = proc_macro2::Span::mixed_site());
    let disabled_ident = format_ident!("__ffrt_disabled", span = proc_macro2::Span::mixed_site());
    let futures_ident = format_ident!("__ffrt_futures", span = proc_macro2::Span::mixed_site());
    let start_ident = format_ident!("__ffrt_start", span = proc_macro2::Span::mixed_site());
    let cx_ident = format_ident!("__ffrt_cx", span = proc_macro2::Span::mixed_site());
    let offset_ident = format_ident!("__ffrt_offset", span = proc_macro2::Span::mixed_site());
    let future_ident = format_ident!("__ffrt_future", span = proc_macro2::Span::mixed_site());
    let output_ident = format_ident!("__ffrt_output", span = proc_macro2::Span::mixed_site());
    quote!({
        enum #output_enum<#(#types),*> { #(#variants(#types),)* Disabled }
        #[allow(unused_mut)]
        let mut #result_ident = {
            // Evaluate every precondition before constructing any future.
            let mut #disabled_ident = [#(!(#conditions)),*];
            let #futures_ident = (#(::core::future::IntoFuture::into_future(#futures),)*);
            let mut #futures_ident = ::core::pin::pin!(#futures_ident);
            let #start_ident = #start;
            ::core::future::poll_fn(|#cx_ident| {
                for #offset_ident in 0..#count {
                    #[allow(clippy::modulo_one)]
                    match (#start_ident + #offset_ident) % #count {
                        #(#numbers => {
                            if #disabled_ident[#numbers] { continue; }
                            // Tuple fields remain pinned until the poll scope ends.
                            let #future_ident = unsafe { #futures_ident.as_mut().map_unchecked_mut(|tuple| &mut tuple.#indexes) };
                            if let ::core::task::Poll::Ready(#output_ident) = ::core::future::Future::poll(#future_ident, #cx_ident) {
                                #disabled_ident[#numbers] = true;
                                #[allow(unused_variables, unreachable_patterns)]
                                match &#output_ident { #checked_patterns => {}, _ => continue }
                                return ::core::task::Poll::Ready(#output_enum::#variants(#output_ident));
                            }
                        },)*
                        _ => unreachable!(),
                    }
                }
                if #disabled_ident.iter().all(|disabled| *disabled) {
                    ::core::task::Poll::Ready(#output_enum::Disabled)
                } else { ::core::task::Poll::Pending }
            }).await
        };
        // Futures and their borrows are dropped before executing a handler.
        // User await/break/return/? therefore retain their original scope.
        #[allow(unreachable_patterns)]
        match #result_ident {
            #(#output_enum::#variants(#patterns) => #handlers,)*
            #output_enum::Disabled => #otherwise,
            _ => unreachable!("selected pattern changed"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_guards_patterns_and_async_handlers() {
        let input = quote!((start); biased; Some(mut x) = future, if enabled => { x += work().await; }, else => 0);
        assert!(syn::parse2::<Select>(input).is_ok());
    }
    #[test]
    fn rejects_more_than_64_branches() {
        let branches = (0..65).map(|_| quote!(_ = future => (),));
        assert!(syn::parse2::<Select>(quote!((start); #(#branches)*)).is_err());
    }
    #[test]
    fn requires_else_last() {
        assert!(syn::parse2::<Select>(quote!((start); else => (), _ = future => ())).is_err());
    }
}
