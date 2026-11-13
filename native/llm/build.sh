#!/bin/sh
# Builds luma-llm (llama.cpp, CPU only, static) for webOS and fetches the
# headline model. Both downloads are pinned by SHA-256.
#
#   native/llm/build.sh            webOS (ARM) build into .build/llm
#   native/llm/build.sh host       desktop build into .build/llm-host
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
CACHE="$ROOT/.build/llm-cache"
mkdir -p "$CACHE"

# llama.cpp as vendored by llama-cpp-python 0.3.36 (PyPI sdist).
SRC_URL=https://files.pythonhosted.org/packages/ec/e9/e7de2b0463ea3ffbf0ede6cb21b58c1258a8f6521aae45ca773a59fe7cf3/llama_cpp_python-0.3.36.tar.gz
SRC_SHA=832db0699007f1be95a7e41ef12e88926b02ba836461e36a36372db2760c1a2e
# SmolLM2-360M-Instruct, Q4_K_M (Apache-2.0), bartowski's GGUF at a fixed revision.
MODEL_URL=https://huggingface.co/bartowski/SmolLM2-360M-Instruct-GGUF/resolve/7be6f65f1db715fe5dc5a4634c0d459b4eed42ec/SmolLM2-360M-Instruct-Q4_K_M.gguf
MODEL_SHA=2fa3f013dcdd7b99f9b237717fa0b12d75bbb89984cc1274be1471a465bac9c2
MODEL_NAME=smollm2-360m-instruct-q4_k_m.gguf

fetch() { # url sha file
    if [ -f "$3" ] && echo "$2  $3" | sha256sum -c - >/dev/null 2>&1; then return; fi
    curl -fsSL --retry 3 -o "$3.part" "$1"
    echo "$2  $3.part" | sha256sum -c - >/dev/null || { echo "Checksum mismatch: $1" >&2; rm -f "$3.part"; exit 1; }
    mv "$3.part" "$3"
}

fetch "$SRC_URL" "$SRC_SHA" "$CACHE/llama-cpp-python.tar.gz"
fetch "$MODEL_URL" "$MODEL_SHA" "$CACHE/$MODEL_NAME"
LLAMA="$CACHE/llama.cpp-$SRC_SHA"
if [ ! -f "$LLAMA/include/llama.h" ]; then
    rm -rf "$CACHE/unpack" && mkdir -p "$CACHE/unpack"
    tar xzf "$CACHE/llama-cpp-python.tar.gz" -C "$CACHE/unpack"
    rm -rf "$LLAMA" && mv "$CACHE/unpack/llama_cpp_python-0.3.36/vendor/llama.cpp" "$LLAMA"
    rm -rf "$CACHE/unpack"
fi

COMMON="-DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_NATIVE=OFF -DGGML_OPENMP=OFF \
 -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF \
 -DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_TOOLS=OFF -DLLAMA_BUILD_COMMON=OFF"

if [ "${1:-}" = host ]; then
    OUT="$ROOT/.build/llm-host"
    # shellcheck disable=SC2086
    cmake -S "$LLAMA" -B "$OUT/cmake" $COMMON >"$OUT.cmake.log" 2>&1 || { tail -20 "$OUT.cmake.log"; exit 1; }
    cmake --build "$OUT/cmake" -j"$(nproc)" --target llama >/dev/null
    CXX=c++
    EXTRA=""
else
    if [ -z "${WEBOS_NDK:-}" ]; then WEBOS_NDK="$ROOT/.build/toolchain/arm-webos-linux-gnueabi_sdk-buildroot"; fi
    SYSROOT="$WEBOS_NDK/arm-webos-linux-gnueabi/sysroot"
    CC="$WEBOS_NDK/bin/arm-webos-linux-gnueabi-gcc"
    CXX="$WEBOS_NDK/bin/arm-webos-linux-gnueabi-g++"
    OUT="$ROOT/.build/llm"
    mkdir -p "$OUT"
    # The toolchain's defaults are ARMv7-A with NEON, soft-float calls.
    # shellcheck disable=SC2086
    cmake -S "$LLAMA" -B "$OUT/cmake" $COMMON \
        -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR=armv7-a \
        -DCMAKE_C_COMPILER="$CC" -DCMAKE_CXX_COMPILER="$CXX" -DCMAKE_SYSROOT="$SYSROOT" \
        -DCMAKE_C_FLAGS=-Wno-psabi -DCMAKE_CXX_FLAGS=-Wno-psabi \
        -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER >"$OUT.cmake.log" 2>&1 || { tail -20 "$OUT.cmake.log"; exit 1; }
    cmake --build "$OUT/cmake" -j"$(nproc)" --target llama >/dev/null
    EXTRA="--sysroot=$SYSROOT -static-libstdc++ -static-libgcc -Wno-psabi -s"
fi
mkdir -p "$OUT"
LIBS=$(find "$OUT/cmake" -name 'libllama.a' -o -name 'libggml*.a' | sort -r | tr '\n' ' ')
# shellcheck disable=SC2086
"$CXX" $EXTRA -O2 -std=c++17 -I"$LLAMA/include" -I"$LLAMA/ggml/include" \
    "$ROOT/native/llm/luma_llm.cpp" -o "$OUT/luma-llm" \
    -Wl,--start-group $LIBS -Wl,--end-group -lpthread -ldl -lm
ln -f "$CACHE/$MODEL_NAME" "$OUT/$MODEL_NAME"
cp "$LLAMA/LICENSE" "$OUT/llama.cpp-LICENSE"
file "$OUT/luma-llm" 2>/dev/null || ls -l "$OUT/luma-llm"
