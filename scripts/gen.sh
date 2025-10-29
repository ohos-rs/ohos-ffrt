#!/bin/bash

script_dir=$(dirname "$0")

header_dir=${ohos_sdk_native}/sysroot/usr/include/ffrt

bindgen ${script_dir}/wrapper.h \
        -o ./crates/sys/src/lib.rs \
        --allowlist-function 'ffrt.*' \
        --allowlist-var 'ffrt.*' \
        --allowlist-type 'ffrt.*' \
        --raw-line '#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]' \
        -- -I${header_dir}