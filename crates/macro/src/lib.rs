use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Ident, ItemFn, LitInt, LitStr, ReturnType, Token, Type, parse::Parse, parse::ParseStream,
    parse_quote,
};

#[derive(Default)]
struct MacroArgs {
    qos: Option<String>,
    priority: Option<String>,
    name: Option<String>,
    delay: Option<u64>,
    stack_size: Option<u64>,
}

impl Parse for MacroArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = MacroArgs::default();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            let key_str = key.to_string();
            match key_str.as_str() {
                "qos" => {
                    let value: LitStr = input.parse()?;
                    if !value.value().eq_ignore_ascii_case("inherit")
                        && !value.value().eq_ignore_ascii_case("background")
                        && !value.value().eq_ignore_ascii_case("utility")
                        && !value.value().eq_ignore_ascii_case("default")
                        && !value.value().eq_ignore_ascii_case("userinitiated")
                    {
                        return Err(syn::Error::new_spanned(
                            value,
                            "qos value must be one of: Inherit, Background, Utility, Default, UserInitiated",
                        ));
                    }
                    args.qos = Some(value.value());
                }
                "priority" => {
                    let value: LitStr = input.parse()?;
                    if !value.value().eq_ignore_ascii_case("immediate")
                        && !value.value().eq_ignore_ascii_case("high")
                        && !value.value().eq_ignore_ascii_case("low")
                        && !value.value().eq_ignore_ascii_case("idle")
                    {
                        return Err(syn::Error::new_spanned(
                            value,
                            "priority value must be one of: Immediate, High, Low, Idle",
                        ));
                    }
                    args.priority = Some(value.value());
                }
                "name" => {
                    let value: LitStr = input.parse()?;
                    args.name = Some(value.value());
                }
                "delay" => {
                    let value: LitInt = input.parse()?;
                    args.delay = Some(value.base10_parse()?);
                }
                "stack_size" => {
                    let value: LitInt = input.parse()?;
                    args.stack_size = Some(value.base10_parse()?);
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        key,
                        format!("unknown attribute: {}", key_str),
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

#[proc_macro_attribute]
pub fn ffrt(args: TokenStream, input: TokenStream) -> TokenStream {
    let macro_args = syn::parse_macro_input!(args as MacroArgs);

    convert(macro_args, input.into())
        .unwrap_or_else(|err| err.into_compile_error())
        .into()
}

fn convert(
    macro_args: MacroArgs,
    input: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream, syn::Error> {
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
    if let ReturnType::Type(_, ty) = func_output
        && let Type::Path(type_path) = &**ty
        && let Some(segment) = type_path.path.segments.last()
        && segment.ident == "Result"
        && !is_napi_ohos_path(&type_path.path)
    {
        return Err(syn::Error::new_spanned(
            ty,
            "ffrt macro requires napi_ohos::Result, not std::result::Result or other Result types",
        ));
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

    // Build TaskAttr initialization code
    let attr_setup = build_attr_setup(&macro_args)?;

    // Determine which spawn method to use
    let spawn_call = if macro_args.has_any_attr() {
        quote! {
            let attr = {
                use ohos_ext::{TaskAttr, Qos, TaskPriority};
                #attr_setup
            };
            env.spawn_local_with_attr(attr, async move #async_body)
        }
    } else {
        quote! {
            env.spawn_local(async move #async_body)
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

            #spawn_call
        }
    })
}

fn build_attr_setup(args: &MacroArgs) -> Result<proc_macro2::TokenStream, syn::Error> {
    let mut setup_code = quote! {
        let attr = TaskAttr::default();
    };

    // Set QoS if provided
    if let Some(ref qos) = args.qos {
        let qos_variant = match qos.as_str() {
            "Inherit" => quote!(Qos::Inherit),
            "Background" => quote!(Qos::Background),
            "Utility" => quote!(Qos::Utility),
            "Default" => quote!(Qos::Default),
            "UserInitiated" => quote!(Qos::UserInitiated),
            _ => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Invalid QoS value: {}. Valid values are: Inherit, Background, Utility, Default, UserInitiated",
                        qos
                    ),
                ));
            }
        };
        setup_code.extend(quote! {
            attr.set_qos(#qos_variant);
        });
    }

    // Set priority if provided
    if let Some(ref priority) = args.priority {
        let priority_variant = match priority.as_str() {
            "Immediate" => quote!(TaskPriority::Immediate),
            "High" => quote!(TaskPriority::High),
            "Low" => quote!(TaskPriority::Low),
            "Idle" => quote!(TaskPriority::Idle),
            _ => {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!(
                        "Invalid priority value: {}. Valid values are: Immediate, High, Low, Idle",
                        priority
                    ),
                ));
            }
        };
        setup_code.extend(quote! {
            attr.set_priority(#priority_variant);
        });
    }

    // Set name if provided
    if let Some(ref name) = args.name {
        setup_code.extend(quote! {
            attr.set_name(#name);
        });
    }

    // Set delay if provided
    if let Some(delay) = args.delay {
        setup_code.extend(quote! {
            attr.set_delay(#delay);
        });
    }

    // Set stack_size if provided
    if let Some(stack_size) = args.stack_size {
        setup_code.extend(quote! {
            attr.set_stack_size(#stack_size);
        });
    }

    setup_code.extend(quote! {
        attr
    });

    Ok(setup_code)
}

impl MacroArgs {
    fn has_any_attr(&self) -> bool {
        self.qos.is_some()
            || self.priority.is_some()
            || self.name.is_some()
            || self.delay.is_some()
            || self.stack_size.is_some()
    }
}

fn is_result_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
        && segment.ident == "Result"
    {
        return is_napi_ohos_path(&type_path.path);
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
    path_str == "napi_ohos::Result"
        || (path.segments.len() == 1 && path.segments[0].ident == "Result")
}

fn extract_result_inner_type(ty: &Type) -> Option<Type> {
    if let Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
        && segment.ident == "Result"
        && is_napi_ohos_path(&type_path.path)
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
    {
        return Some(inner_ty.clone());
    }
    None
}
