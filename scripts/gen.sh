#!/bin/bash

set -e  # Exit on error

script_dir=$(dirname "$0")

header_dir=${ohos_sdk_native}/sysroot/usr/include

echo "Generating Rust bindings with bindgen..."
bindgen ${script_dir}/wrapper.h \
        -o ./crates/sys/src/lib.rs \
        --translate-enum-integer-types \
        --default-alias-style 'type_alias' \
        --allowlist-function 'ffrt.*' \
        --allowlist-var 'ffrt.*' \
        --allowlist-type 'ffrt.*' \
        --no-layout-tests \
        --raw-line '#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]' \
        --raw-line '#![allow(clippy::missing_safety_doc)]' \
        --raw-line '
#[link(name = "ffrt.z")]' \
        --raw-line 'unsafe extern "C" {}' \
        -- -I${header_dir} -I${header_dir}/aarch64-linux-ohos \

echo "Adding feature gates based on @since annotations..."

python3 ${script_dir}/add_feature_gates.py \
        ./crates/sys/src/lib.rs

echo "Done! Generated bindings with API version feature gates."