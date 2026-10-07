#!/usr/bin/env bash
# Execute Rust regressions against the real upstream FFRT library on Linux.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${FFRT_TEST_CACHE:-${ROOT}/target/ffrt-test-backend}"
FFRT_REV=1096b676295daa71541c58b3a287a9d1d0bd1603
SECUREC_REV=2ae82839ecaaa7d031e66ccbd2076d671acfd615
mkdir -p "${CACHE}"

checkout() {
    local url="$1" revision="$2" destination="$3"
    if [[ ! -d "${destination}/.git" ]]; then
        git init -q "${destination}"
        git -C "${destination}" remote add origin "${url}"
    fi
    if [[ "$(git -C "${destination}" rev-parse HEAD 2>/dev/null || true)" != "${revision}" ]]; then
        git -C "${destination}" fetch --depth 1 origin "${revision}"
        git -C "${destination}" checkout --detach FETCH_HEAD
    fi
}

# Overrides permit an offline run using existing upstream source checkouts.
if [[ -z "${FFRT_TEST_SOURCE:-}" ]]; then
    checkout https://github.com/openharmony/resourceschedule_ffrt.git "${FFRT_REV}" "${CACHE}/ffrt"
fi
if [[ -z "${FFRT_TEST_SECUREC:-}" ]]; then
    checkout https://github.com/openharmony/third_party_bounds_checking_function.git "${SECUREC_REV}" "${CACHE}/securec"
fi
cmake -S "${FFRT_TEST_SOURCE:-${CACHE}/ffrt}" -B "${CACHE}/build" \
    -DCMAKE_BUILD_TYPE=Release -DFFRT_EXAMPLE=OFF \
    -DSECUREC_PATH="${FFRT_TEST_SECUREC:-${CACHE}/securec}"
cmake --build "${CACHE}/build" --parallel "${FFRT_BUILD_JOBS:-2}"
ln -sf libffrt.so "${CACHE}/build/src/libffrt.z.so"
export RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }-Lnative=${CACHE}/build/src"
export RUSTDOCFLAGS="${RUSTDOCFLAGS:+${RUSTDOCFLAGS} }-Lnative=${CACHE}/build/src"
export LD_LIBRARY_PATH="${CACHE}/build/src:${CACHE}/build${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
cd "${ROOT}"
cargo test --locked -p ffrt-macros -p ohos-ext-macro
cargo test --locked -p ffrt --all-features
cargo test --locked -p ffrt --no-default-features --test regressions
cargo run --locked -p ffrt --all-features --example qemu_smoke
cargo run --locked -p tokio-compat
