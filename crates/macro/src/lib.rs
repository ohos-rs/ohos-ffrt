use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, ReturnType, Type, parse_quote};

#[proc_macro_attribute]
pub fn ffrt(_args: TokenStream, input: TokenStream) -> TokenStream {
    convert(input.into())
        .unwrap_or_else(|err| err.into_compile_error())
        .into()
}

fn convert(input: proc_macro2::TokenStream) -> Result<proc_macro2::TokenStream, syn::Error> {
    let func = syn::parse2::<ItemFn>(input)?;

    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            func,
            "ffrt macro only supports async functions",
        ));
    }

    // Extract function metadata
    let func_name = &func.sig.ident;
    let func_vis = &func.vis;
    let func_attrs = &func.attrs;
    let func_inputs = &func.sig.inputs;
    let func_body = &func.block;
    let func_output = &func.sig.output;

    // Collect parameter names for the async block
    let mut param_names = Vec::new();
    for input in func_inputs.iter() {
        if let syn::FnArg::Typed(pat_type) = input {
            param_names.push(&pat_type.pat);
        }
    }

    // Check for incorrect Result usage
    if let ReturnType::Type(_, ty) = func_output {
        if let Type::Path(type_path) = &**ty {
            if let Some(segment) = type_path.path.segments.last() {
                if segment.ident == "Result" && !is_napi_ohos_path(&type_path.path) {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "ffrt macro requires napi_ohos::Result, not std::result::Result or other Result types",
                    ));
                }
            }
        }
    }

    // Determine the inner return type (what the async function returns)
    let inner_return_type = match func_output {
        ReturnType::Default => {
            // If no return type, default to ()
            parse_quote!(())
        }
        ReturnType::Type(_, ty) => {
            // Check if it's already a Result type
            if is_result_type(ty) {
                // Extract T from Result<T>
                extract_result_inner_type(ty).unwrap_or_else(|| parse_quote!(()))
            } else {
                // Not a Result, use as-is
                (**ty).clone()
            }
        }
    };

    // Determine if original function returns Result
    let returns_result = match func_output {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => is_result_type(ty),
    };

    // Build the async block body
    let async_body = if returns_result {
        // Already returns Result, use as-is
        quote! {
            #func_body
        }
    } else {
        // Wrap the entire function body result in Ok()
        // For fn() -> T, this becomes Ok({ body })
        // For fn() -> (), this becomes Ok({ body })
        quote! {
            {
                Ok(#func_body)
            }
        }
    };

    // Generate the wrapper function
    Ok(quote! {
        #(#func_attrs)*
        #[napi_derive_ohos::napi]
        #func_vis fn #func_name<'env>(
            env: &'env napi_ohos::Env,
            #func_inputs
        ) -> napi_ohos::Result<napi_ohos::bindgen_prelude::PromiseRaw<'env, #inner_return_type>> {
            use ohos_ext::SpawnLocalExt;

            env.spawn_local(async move #async_body)
        }
    })
}

fn is_result_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        // Check if the last segment is "Result"
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Result" {
                // Check if it's from napi_ohos
                return is_napi_ohos_path(&type_path.path);
            }
        }
    }
    false
}

fn is_napi_ohos_path(path: &syn::Path) -> bool {
    // Accept the following patterns:
    // - napi_ohos::Result
    // - Result (if imported from napi_ohos)
    
    let path_str = path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    
    // Check if it's explicitly napi_ohos::Result or just Result (assumed to be imported)
    path_str == "napi_ohos::Result" || (path.segments.len() == 1 && path.segments[0].ident == "Result")
}

fn extract_result_inner_type(ty: &Type) -> Option<Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Result" && is_napi_ohos_path(&type_path.path) {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                        return Some(inner_ty.clone());
                    }
                }
            }
        }
    }
    None
}
