//
// The one HybridObject: bytes in, one buffer out, converted off the JS thread.
//
// The Rust side (anydoc/mobile) owns the whole contract: it NEVER panics
// across the FFI and never returns a partial result, so this layer is pure
// plumbing - no try/catch theatre, no interpretation of the payload.
//

#pragma once

#include "HybridDocumentConverterSpec.hpp"

#include <cstdint>
#include <memory>
#include <vector>

extern "C" {
// anydoc/mobile/src/lib.rs - the single source of truth for this ABI.
int32_t anydoc_tula_convert(const uint8_t* input, size_t inputLen, uint8_t** out, size_t* outLen);
void anydoc_tula_free(uint8_t* ptr, size_t len);
}

namespace margelo::nitro::tula::anydoc {

class HybridDocumentConverter : public HybridDocumentConverterSpec {
public:
  HybridDocumentConverter() : HybridObject(TAG) {}

  std::shared_ptr<Promise<std::shared_ptr<ArrayBuffer>>>
  convert(const std::shared_ptr<ArrayBuffer>& document) override {
    // Copy the input BEFORE leaving the JS thread: a JS ArrayBuffer's memory
    // is only guaranteed alive while JS can't run, and the conversion runs on
    // Nitro's thread pool.
    auto input = std::make_shared<std::vector<uint8_t>>(
        document->data(), document->data() + document->size());

    return Promise<std::shared_ptr<ArrayBuffer>>::async(
        [input]() -> std::shared_ptr<ArrayBuffer> {
          uint8_t* out = nullptr;
          size_t outLen = 0;
          int32_t code = anydoc_tula_convert(input->data(), input->size(), &out, &outLen);
          if (code != 0 || out == nullptr) {
            // Only reachable through invalid FFI arguments, which this
            // wrapper cannot produce - but never hand JS a null buffer.
            throw std::runtime_error("anydoc_tula_convert failed with code " +
                                     std::to_string(code));
          }
          // Zero-copy: JS reads the Rust allocation directly, and the GC's
          // deleter is the Rust free. One allocation crosses the bridge.
          return ArrayBuffer::wrap(out, outLen,
                                   [out, outLen]() { anydoc_tula_free(out, outLen); });
        });
  }
};

} // namespace margelo::nitro::tula::anydoc
