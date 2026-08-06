//
// The one HybridObject: bytes in, {json string, asset blob} out, converted
// off the JS thread.
//
// The Rust side (anydoc/mobile) owns the whole contract: it NEVER panics
// across the FFI and never returns a partial result. This layer's only job
// beyond plumbing is to peel the JSON off the Rust buffer and bridge it as a
// std::string, so the UTF-8 -> UTF-16 conversion happens in native code
// instead of a JS decode loop (which dominated conversion time on Hermes).
//

#pragma once

#include "HybridDocumentConverterSpec.hpp"
#include "NativeConvertOutput.hpp"

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#ifdef __ANDROID__
#include <android/log.h>
#include <chrono>
#endif

extern "C" {
// anydoc/mobile/src/lib.rs - the single source of truth for this ABI.
int32_t anydoc_tula_convert(const uint8_t* input, size_t inputLen, uint8_t** out, size_t* outLen);
void anydoc_tula_free(uint8_t* ptr, size_t len);
}

namespace margelo::nitro::tula::anydoc {

class HybridDocumentConverter : public HybridDocumentConverterSpec {
public:
  HybridDocumentConverter() : HybridObject(TAG) {}

  std::shared_ptr<Promise<NativeConvertOutput>>
  convert(const std::shared_ptr<ArrayBuffer>& document) override {
    // Copy the input BEFORE leaving the JS thread: a JS ArrayBuffer's memory
    // is only guaranteed alive while JS can't run, and the conversion runs on
    // Nitro's thread pool.
    auto input = std::make_shared<std::vector<uint8_t>>(
        document->data(), document->data() + document->size());

    return Promise<NativeConvertOutput>::async(
        [input]() -> NativeConvertOutput {
          uint8_t* out = nullptr;
          size_t outLen = 0;
#ifdef __ANDROID__
          auto t0 = std::chrono::steady_clock::now();
#endif
          int32_t code = anydoc_tula_convert(input->data(), input->size(), &out, &outLen);
#ifdef __ANDROID__
          // Sizes and duration only - never document content. A JS-measured
          // time far above this line means the resolution queued behind a
          // busy JS thread, not a slow conversion.
          auto nativeMs = std::chrono::duration_cast<std::chrono::milliseconds>(
                              std::chrono::steady_clock::now() - t0)
                              .count();
          __android_log_print(ANDROID_LOG_INFO, "anydoc-native",
                              "convert %zu bytes -> %zu bytes in %lld ms", input->size(),
                              outLen, static_cast<long long>(nativeMs));
#endif
          if (code != 0 || out == nullptr) {
            // Only reachable through invalid FFI arguments, which this
            // wrapper cannot produce - but never hand JS a null result.
            throw std::runtime_error("anydoc_tula_convert failed with code " +
                                     std::to_string(code));
          }

          // Rust buffer layout: [u32 LE json length][JSON UTF-8][asset bytes].
          uint32_t jsonLen = 0;
          if (outLen >= 4) {
            jsonLen = static_cast<uint32_t>(out[0]) | (static_cast<uint32_t>(out[1]) << 8) |
                      (static_cast<uint32_t>(out[2]) << 16) | (static_cast<uint32_t>(out[3]) << 24);
          }
          if (outLen < 4 || jsonLen > outLen - 4) {
            anydoc_tula_free(out, outLen);
            throw std::runtime_error("anydoc_tula_convert returned a malformed buffer");
          }

          std::string json(reinterpret_cast<const char*>(out) + 4, jsonLen);

          // The trailing asset blob stays zero-copy: JS reads the Rust
          // allocation directly, and the GC's deleter is the Rust free. The
          // deleter owns the WHOLE allocation, so it must run exactly once -
          // which ArrayBuffer::wrap guarantees - even when the blob is empty.
          auto assets = ArrayBuffer::wrap(out + 4 + jsonLen, outLen - 4 - jsonLen,
                                          [out, outLen]() { anydoc_tula_free(out, outLen); });
          return NativeConvertOutput(std::move(json), assets);
        });
  }
};

} // namespace margelo::nitro::tula::anydoc
