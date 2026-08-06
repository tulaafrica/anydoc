import type { HybridObject } from 'react-native-nitro-modules'

/**
 * What the native layer hands back, already split:
 *
 * - `json`: the DocumentIR result JSON as a STRING. The Rust side emits one
 *   buffer ([u32 LE json length][JSON UTF-8][asset bytes]); the C++ layer
 *   peels the JSON off and bridges it as a string so the UTF-8 -> UTF-16
 *   conversion happens natively. Decoding half a megabyte of JSON in JS was
 *   the single dominant cost of a conversion on dev-mode Hermes.
 * - `assets`: the trailing asset blob, zero-copy. The JSON's `assets` array
 *   carries each asset's offset/length into THIS buffer.
 */
export interface NativeConvertOutput {
  json: string
  assets: ArrayBuffer
}

/**
 * The one native call: document bytes in, converted on Nitro's thread pool
 * so the JS thread never blocks. NEVER rejects for document reasons — every
 * malformed, encrypted or hostile input resolves to a {"status":"fallback"}
 * payload (anydoc/mobile/src/lib.rs is the single source of truth).
 */
export interface DocumentConverter
  extends HybridObject<{ ios: 'c++'; android: 'c++' }> {
  convert(document: ArrayBuffer): Promise<NativeConvertOutput>
}
